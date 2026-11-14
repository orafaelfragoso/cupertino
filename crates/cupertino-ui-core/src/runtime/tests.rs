use super::*;
use crate::test_support::{group, Group};
use crate::test_support::{
    leaf, options, options_with, row, FakeAdapter, FakeClock, FakeSystem, Leaf, MAX_PIXELS,
};
use crate::{
    Change, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Shortcut, Widget,
};
use crate::{DropOver, DropPlace, DropTarget};
use cupertino_geometry::Rect;
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, Overflow, SizeSpec,
};
use cupertino_ui_platform::{
    Configure, DecorationMode, DropAction, DropAnswer, DropEvent, DropToken, Mods, MouseButton,
    PointerEvent, WindowStates, WmCapabilities,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, ColorTokens, StateVariant, Surface};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::Ordering;

/// Two buttons; pressing `plus` (or Primary+=) counts up, and a `wait` message starts a task.
struct Counter;

#[derive(Default)]
struct Model {
    count: u32,
    done: Vec<u32>,
    background: WindowBackground,
    tokens: Vec<Option<String>>,
    /// Each step of a drag, as it was told.
    drags: Vec<String>,
    /// The windows heard to have presented a frame, in the order heard.
    presented: Vec<String>,
}

impl Application for Counter {
    type Model = Model;
    type Message = String;

    fn init() -> (Model, Vec<Command<String>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: String) -> Vec<Command<String>> {
        match message.as_str() {
            "plus:Target:press" | "plus" => model.count += 1,
            "quit" => return vec![Command::Exit],
            "panel" => {
                model.background = match model.background {
                    WindowBackground::Theme => WindowBackground::None,
                    _ => WindowBackground::Theme,
                }
            }
            "menu" => {
                model.background = WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius: 8.0,
                    alpha: 1.0,
                }
            }
            "half" => {
                model.background = WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius: 8.0,
                    alpha: 0.5,
                }
            }
            "nan" => {
                model.background = WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius: 8.0,
                    alpha: f64::NAN,
                }
            }
            "corners" | "rounder corners" => {
                let radius = if message == "corners" { 8.0 } else { 16.0 };
                model.background = WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius,
                    alpha: 1.0,
                }
            }
            "catcher" => model.background = WindowBackground::Solid(Color::new(0.0, 0.0, 0.0, 0.0)),
            "scrim" => model.background = WindowBackground::Solid(SCRIM),
            "close" => return vec![Command::CloseWindow(WindowKey("main".into()))],
            "wait" => return vec![Command::Perform(Box::pin(async { "done 7".to_string() }))],
            "launch" | "launch from gone" => {
                let window = match message.as_str() {
                    "launch" => "main",
                    _ => "gone",
                };
                return vec![Command::ActivationToken {
                    window: WindowKey(window.into()),
                    app_id: Some("test.viewer".into()),
                    on_token: Box::new(|token| format!("token {}", token.unwrap_or_default())),
                }];
            }
            "drag" | "drag from gone" => {
                let window = match message.as_str() {
                    "drag" => "main",
                    _ => "gone",
                };
                return vec![Command::StartDrag(Drag {
                    window: WindowKey(window.into()),
                    data: vec![ClipboardData {
                        mime: "text/uri-list".into(),
                        bytes: b"file:///a\r\n".to_vec(),
                    }],
                    actions: [DropAction::Copy, DropAction::Move].into(),
                    image: Some(DragPreview {
                        content: Element::new(leaf("ghost")),
                        size: Size::new(30.2, 20.0),
                        hotspot: Point::new(5.5, 4.0),
                    }),
                    on_progress: Box::new(|progress| format!("dragged {progress:?}")),
                })];
            }
            m if m.starts_with("dragged ") => model.drags.push(m[8..].to_owned()),
            m if m.starts_with("await ") => {
                return vec![Command::OnPresented {
                    window: WindowKey(m[6..].to_owned().into()),
                    message: format!("presented {}", &m[6..]),
                }]
            }
            m if m.starts_with("presented ") => model.presented.push(m[10..].to_owned()),
            m if m.starts_with("later ") => {
                let (nanos, name) = m[6..].split_once(' ').unwrap();
                return vec![Command::After {
                    delay: Duration::from_nanos(nanos.parse().unwrap()),
                    message: format!("due {name}"),
                }];
            }
            m if m.starts_with("due ") => model.presented.push(m.to_owned()),
            m if m.starts_with("token ") => model
                .tokens
                .push(Some(m[6..].to_owned()).filter(|t| !t.is_empty())),
            m if m.starts_with("done ") => model.done.push(m[5..].parse().unwrap()),
            _ => {}
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<String>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: format!("Count {}", model.count),
                app_id: "test.counter".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new().with(
                Shortcut {
                    mods: Mods::PRIMARY,
                    keysym: Keysym(0x3d),
                },
                "plus".to_string(),
            ),
            on_close: None,
            popup: None,
            background: model.background,
            content: Element::new(row(vec![
                Element::new(Leaf {
                    drawn: true,
                    ..leaf("plus")
                })
                .key("plus"),
                Element::new(Leaf {
                    secret: true,
                    animated: true,
                    drawn: true,
                    ..leaf("minus")
                })
                .key("minus"),
            ])),
            menus: None,
        }]
    }
}

use crate::Element;

/// The colour of a window that dims what is under it.
const SCRIM: Color = Color::new(0.0, 0.0, 0.0, 0.3);

type Rt = Runtime<Counter, FakeSystem>;

fn steps() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}

fn start() -> (Rt, WindowId, Rc<RefCell<Vec<DamageRegion>>>) {
    start_with(|_| {})
}

fn start_with(
    setup: impl FnOnce(&mut RuntimeOptions),
) -> (Rt, WindowId, Rc<RefCell<Vec<DamageRegion>>>) {
    let damage = Rc::default();
    let mut options = options(&damage);
    setup(&mut options);
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    (rt, window, damage)
}

fn configure(rt: &mut Rt, window: WindowId, id: u32) {
    configure_at(rt, window, id, Size::new(200.0, 100.0));
}

fn configure_at<A: Application>(
    rt: &mut Runtime<A, FakeSystem>,
    window: WindowId,
    id: u32,
    size: Size,
) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Configure(Configure {
            id: ConfigureId(id),
            size: Some(size),
            states: WindowStates {
                activated: true,
                ..WindowStates::default()
            },
            bounds: None,
            server_decorations: true,
            wm_capabilities: WmCapabilities {
                window_menu: true,
                maximize: true,
                fullscreen: true,
                minimize: true,
            },
        }),
    );
    rt.step(steps()).unwrap();
}

fn click<A: Application>(rt: &mut Runtime<A, FakeSystem>, window: WindowId, x: f64) {
    for pressed in [true, false] {
        rt.window_system_mut().send(
            window,
            WindowEvent::Pointer(
                PointerEvent::Button {
                    position: Point::new(x, 5.0),
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: cupertino_ui_platform::Mods::NONE,
                },
                Timestamp(0),
            ),
        );
    }
    rt.step(steps()).unwrap();
}

fn key(rt: &mut Rt, window: WindowId, keysym: u32, mods: Mods) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed: true,
                repeat: false,
                mods,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
}

#[test]
fn a_window_is_drawn_only_after_its_first_configure_which_the_frame_acknowledges() {
    let (mut rt, window, damage) = start();
    assert_eq!(rt.window_system().created.len(), 1);
    assert!(rt.window_system().presented.is_empty());
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Waiting(Pending {
            frames: 1,
            ..Pending::default()
        })
    );

    configure(&mut rt, window, 9);

    assert_eq!(
        rt.window_system().presented,
        vec![(window, Some(ConfigureId(9)))]
    );
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
    assert_eq!(rt.step(steps()).unwrap(), StepStatus::Idle);
}

/// A window that opens active shows the active colours from its first frame, so no crossing follows and the next frame
/// paints nothing. Made active again once it was drawn, its colours cross: the next frame paints it whole.
#[test]
fn a_window_opening_active_is_active_at_once_and_one_activated_later_crosses_its_colours() {
    let (mut rt, window, damage) = start();
    configure(&mut rt, window, 1);
    let drawn = damage.borrow().len();
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(
        damage.borrow().len(),
        drawn,
        "no crossing after opening active"
    );

    rt.window_system_mut()
        .send(window, WindowEvent::Focus(false));
    rt.step(steps()).unwrap();
    rt.window_system_mut()
        .send(window, WindowEvent::Focus(true));
    rt.step(steps()).unwrap();
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(2)));
    rt.step(steps()).unwrap();
    assert_eq!(
        damage.borrow().last(),
        Some(&DamageRegion::Full),
        "the colours cross"
    );
}

/// A window of one colour (here a transparent one that only takes input) is shown by the window system without a frame:
/// nothing is rendered or presented, its content still takes the pointer, and it is shown again only to answer a
/// configure or to show another colour; a new scale or a change of its content shows nothing new. Back on the theme's
/// background it draws a whole frame.
#[test]
fn a_solid_window_is_shown_without_a_frame_and_again_only_for_a_configure_or_another_colour() {
    let (mut rt, window, damage) = start();
    rt.window_system_mut().solid = true;
    rt.messages.push_back("catcher".into());
    rt.step(steps()).unwrap();
    configure(&mut rt, window, 4);
    let clear = Color::new(0.0, 0.0, 0.0, 0.0);
    assert_eq!(
        rt.window_system().solids,
        vec![(window, clear, Some(ConfigureId(4)))]
    );
    assert!(rt.window_system().presented.is_empty(), "no frame");
    assert!(damage.borrow().is_empty(), "nothing rendered");
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);

    click(&mut rt, window, 5.0);
    assert_eq!(rt.model().count, 1, "the content takes the pointer");
    rt.window_system_mut().send(window, WindowEvent::Scale(1.5));
    rt.step(steps()).unwrap();
    frame(&mut rt);
    assert_eq!(rt.window_system().solids.len(), 1, "nothing new to show");

    configure_at(&mut rt, window, 5, Size::new(100.0, 200.0));
    assert_eq!(
        rt.window_system().solids[1],
        (window, clear, Some(ConfigureId(5)))
    );
    frame(&mut rt);
    rt.messages.push_back("scrim".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().solids[2], (window, SCRIM, None));
    assert!(rt.window_system().presented.is_empty());
    assert!(damage.borrow().is_empty());

    // a refused present waits for the window system, then shows the colour
    frame(&mut rt);
    rt.window_system_mut().refuse_next = true;
    rt.messages.push_back("catcher".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().solids.len(), 3);
    rt.window_system_mut().release(window);
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().solids[3], (window, clear, None));

    frame(&mut rt);
    rt.messages.push_back("panel".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().presented, vec![(window, None)]);
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
    assert_eq!(rt.window_system().solids.len(), 4);
}

/// One window whose content lays the popover's glass under itself, on the theme's background or on one colour as the
/// model says.
struct Glassy;

impl Application for Glassy {
    type Model = WindowBackground;
    type Message = WindowBackground;

    fn init() -> (WindowBackground, Vec<Command<WindowBackground>>) {
        (WindowBackground::Theme, Vec::new())
    }

    fn update(
        model: &mut WindowBackground,
        message: WindowBackground,
    ) -> Vec<Command<WindowBackground>> {
        *model = message;
        Vec::new()
    }

    fn view(model: &WindowBackground) -> Vec<Window<WindowBackground>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Glass".into(),
                app_id: "test.glass".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: *model,
            content: Element::new(GlassPane),
            menus: None,
        }]
    }
}

/// A 50 x 20 pane on the popover's glass.
struct GlassPane;

impl<M: 'static> Widget<M> for GlassPane {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(50.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.material(Surface::Popover, 6.0, 1.0);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A window whose content stood on glass and turns one colour takes the glass away with the colour, since its content
/// paints nothing to stand on it; back on the theme's background it asks for the glass again.
#[test]
fn a_window_turning_solid_takes_its_content_s_glass_away_and_gets_it_back_when_drawn_again() {
    let damage = Rc::default();
    let mut system = FakeSystem::default();
    system.effects = true;
    system.solid = true;
    let mut rt = Runtime::<Glassy, FakeSystem>::new(system, options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let materials = |rt: &Runtime<Glassy, FakeSystem>| -> Vec<usize> {
        rt.window_system()
            .requests
            .iter()
            .filter_map(|(w, r)| match r {
                WindowRequest::Materials(m) if *w == window => Some(m.len()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(materials(&rt), [1], "the pane's glass");
    assert_eq!(rt.window_system().presented.len(), 1);

    let frame = |rt: &mut Runtime<Glassy, FakeSystem>| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    rt.messages.push_back(WindowBackground::INPUT_ONLY);
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().solids.len(), 1);
    assert_eq!(materials(&rt), [1, 0], "no glass under the colour");

    frame(&mut rt);
    rt.messages.push_back(WindowBackground::Theme);
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().presented.len(), 2);
    assert_eq!(materials(&rt), [1, 0, 1], "the glass again");
}

/// One window whose 50 x 20 pane stands on the popover's glass at the alpha the model says.
struct FadingGlass;

impl Application for FadingGlass {
    type Model = u8;
    type Message = u8;

    fn init() -> (u8, Vec<Command<u8>>) {
        (100, Vec::new())
    }

    fn update(model: &mut u8, message: u8) -> Vec<Command<u8>> {
        *model = message;
        Vec::new()
    }

    fn view(model: &u8) -> Vec<Window<u8>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Fading glass".into(),
                app_id: "test.fading".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::Theme,
            content: Element::new(FadingPane(*model)),
            menus: None,
        }]
    }
}

/// A 50 x 20 pane on the popover's glass at an alpha in percent.
struct FadingPane(u8);

impl<M: 'static> Widget<M> for FadingPane {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.0 == self.0 {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(50.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.material(Surface::Popover, 6.0, f64::from(self.0) / 100.0);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Glass that fades (or moves, or grows) is drawn by the window system from the regions each frame sends: the frame
/// commits with them, but the window's own pixels, which did not change, are not drawn again. Before, every frame of a
/// fade repainted the whole window.
#[test]
fn a_frame_whose_glass_alone_changed_commits_the_regions_without_repainting_the_window() {
    let damage = Rc::default();
    let mut system = FakeSystem::default();
    system.effects = true;
    let mut rt = Runtime::<FadingGlass, FakeSystem>::new(system, options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    assert_eq!(rt.window_system().presented.len(), 1);
    assert_eq!(
        damage.borrow().as_slice(),
        [DamageRegion::Full],
        "the first frame"
    );
    let alphas = |rt: &Runtime<FadingGlass, FakeSystem>| -> Vec<f64> {
        rt.window_system()
            .requests
            .iter()
            .filter_map(|(w, r)| match r {
                WindowRequest::Materials(m) if *w == window => Some(m[0].alpha),
                _ => None,
            })
            .collect()
    };
    for (n, alpha) in [80u8, 60, 40, 20].into_iter().enumerate() {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(n as u64 + 1)));
        rt.step(steps()).unwrap();
        rt.messages.push_back(alpha);
        rt.step(steps()).unwrap();
        assert_eq!(
            rt.window_system().presented.len(),
            n + 2,
            "the frame at {alpha}% commits its glass"
        );
        assert_eq!(alphas(&rt).last().copied(), Some(f64::from(alpha) / 100.0));
        let last = damage.borrow().last().cloned();
        assert!(
            matches!(&last, Some(DamageRegion::Rects(r)) if r.is_empty()),
            "the frame at {alpha}% repaints no pixel of the window: {last:?}"
        );
    }
}

/// One window showing a block that fades out by itself over a second of the runtime's clock.
struct SelfFading;

impl Application for SelfFading {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Fading".into(),
                app_id: "test.fading".into(),
                size: Size::new(100.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::Theme,
            content: Element::new(FadeOut),
            menus: None,
        }]
    }
}

/// A 40 x 40 block whose alpha falls from 1 to 0 over the first second of the clock. It asks for animation frames from
/// its paint and handles no event, as a band that fades once released does.
struct FadeOut;

impl<M: 'static> Widget<M> for FadeOut {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(40.0, 40.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let t = cx.now().0 as f64 / 1e9;
        if t < 1.0 {
            cx.request_animation_frame();
        }
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 40.0, 40.0)),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, (1.0 - t).max(0.0) as f32)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A node that animates from its paint draws every frame of its animation: each frame callback paints it again at the
/// clock's time and presents, until it stops asking. Before, the node kept its cached paint at the first callback,
/// presented nothing, and so asked for no more frames: the fade stood still at its first step.
#[test]
fn a_node_animating_from_its_paint_paints_again_at_every_frame_until_it_stops_asking() {
    let clock = Arc::new(FakeClock::default());
    let damage = Rc::default();
    let mut options = options(&damage);
    options.clock = clock.clone();
    let mut rt = Runtime::<SelfFading, FakeSystem>::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(100.0, 100.0));
    assert_eq!(rt.window_system().presented.len(), 1);
    let frame = 16_666_667u64;
    let mut n = 0;
    while n * frame < 1_000_000_000 {
        n += 1;
        clock.0.store(n * frame, Ordering::SeqCst);
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(n)));
        rt.step(steps()).unwrap();
        assert_eq!(
            rt.window_system().presented.len() as u64,
            n + 1,
            "frame {n} of the fade presents"
        );
    }
    clock.0.store(2_000_000_000, Ordering::SeqCst);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1_000)));
    rt.step(steps()).unwrap();
    let presented = rt.window_system().presented.len();
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1_001)));
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.window_system().presented.len(),
        presented,
        "once faded it stops"
    );
}

/// One window showing a block whose alpha falls in four steps over a second of the runtime's clock.
struct SteppedFading;

impl Application for SteppedFading {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let mut windows = SelfFading::view(&());
        windows[0].content = Element::new(SteppedFade);
        windows
    }
}

/// A 40 x 40 block whose alpha drops by a quarter every 250 ms until the clock reaches a second: most frames of its
/// animation paint the same pixels as the one before, as a slow fade's frames do between two levels of a colour.
struct SteppedFade;

impl<M: 'static> Widget<M> for SteppedFade {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(40.0, 40.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let t = cx.now().0 as f64 / 1e9;
        if t < 1.0 {
            cx.request_animation_frame();
        }
        let step = (t * 4.0).floor().min(4.0);
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 40.0, 40.0)),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, (1.0 - step / 4.0) as f32)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// An animation whose next frame paints the same pixels as the last keeps asking for frames until it reports done:
/// the frame commits with no damage, so the compositor paces the next one. Before, such a frame sent nothing and asked
/// for nothing, and the animation stood still at its first unchanged step.
#[test]
fn an_animation_keeps_its_frames_through_steps_that_paint_the_same_pixels() {
    let clock = Arc::new(FakeClock::default());
    let damage = Rc::default();
    let mut options = options(&damage);
    options.clock = clock.clone();
    let mut rt = Runtime::<SteppedFading, FakeSystem>::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(100.0, 100.0));
    let frame = 16_666_667u64;
    let mut n = 0;
    while n * frame < 1_000_000_000 {
        n += 1;
        clock.0.store(n * frame, Ordering::SeqCst);
        let asked = rt.window_system().frames_requested;
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(n)));
        rt.step(steps()).unwrap();
        assert!(
            rt.window_system().frames_requested > asked,
            "frame {n} of the fade asks for the next"
        );
    }
    let alpha = |rt: &Runtime<SteppedFading, FakeSystem>| {
        rt.snapshot(window)
            .unwrap()
            .display_list
            .nodes
            .iter()
            .flat_map(|node| &node.ops)
            .find_map(|op| match op {
                PaintOp::Fill {
                    paint: Paint::Solid(c),
                    shape: Shape::Rect(r),
                } if r.size.width == 40.0 => Some(c.a),
                _ => None,
            })
    };
    assert_eq!(alpha(&rt), Some(0.0), "the fade reached its last step");
    let asked = rt.window_system().frames_requested;
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1_000)));
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.window_system().frames_requested,
        asked,
        "once done it asks for no more"
    );
}

/// Where the window system cannot show a colour without a frame, a solid window is drawn like any other: filled with
/// its colour under its content, and not filled when the colour is transparent.
#[test]
fn a_solid_window_the_window_system_cannot_show_is_drawn_filled_with_its_colour() {
    let (mut rt, window, damage) = start();
    rt.messages.push_back("scrim".into());
    rt.step(steps()).unwrap();
    configure(&mut rt, window, 1);
    assert!(rt.window_system().solids.is_empty());
    assert_eq!(
        rt.window_system().presented,
        vec![(window, Some(ConfigureId(1)))]
    );
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
    let first_op = |rt: &Rt| {
        rt.snapshot(window).unwrap().display_list.nodes[0]
            .ops
            .first()
            .cloned()
    };
    assert_eq!(
        first_op(&rt),
        Some(PaintOp::Fill {
            shape: Shape::Rect(cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0)),
            paint: Paint::Solid(SCRIM),
        })
    );

    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.messages.push_back("catcher".into());
    rt.step(steps()).unwrap();
    assert_eq!(first_op(&rt), None, "nothing to fill");
    assert_eq!(rt.window_system().presented.len(), 2);
}

/// The frame drawn for a refused present is shown when the window system resumes, which kept its damage: the frame
/// after it draws no pixel again, yet commits, acknowledging the configure.
#[test]
fn a_refused_frame_waits_for_the_window_system_and_is_shown_when_it_resumes() {
    let (mut rt, window, damage) = start();
    rt.window_system_mut().refuse_next = true;
    configure(&mut rt, window, 3);

    assert!(rt.window_system().presented.is_empty());
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Waiting(Pending {
            frames: 1,
            ..Pending::default()
        }),
        "no retry until the window system says the window may draw"
    );

    rt.window_system_mut().release(window);
    rt.step(steps()).unwrap();

    assert_eq!(
        rt.window_system().presented,
        vec![(window, Some(ConfigureId(3)))]
    );
    assert_eq!(
        damage.borrow().as_slice(),
        [DamageRegion::Full, DamageRegion::Rects(Vec::new())]
    );
    assert_eq!(rt.step(steps()).unwrap(), StepStatus::Idle);
}

#[test]
fn a_focus_move_repaints_only_what_the_focus_changed() {
    let (mut rt, window, damage) = start();
    configure(&mut rt, window, 1);
    let rect = |rt: &Rt, key: &str| {
        let id = rt.find(window, &Key::from(key.to_owned())).unwrap();
        let snapshot = rt.snapshot(window).unwrap();
        snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
    };
    let (plus, minus) = (rect(&rt, "plus"), rect(&rt, "minus"));

    key(&mut rt, window, 0xff09, Mods::NONE);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(
        damage.borrow().last(),
        Some(&DamageRegion::Rects(vec![plus, plus])),
        "where the button was and is; the row draws nothing and the other button what it drew"
    );

    key(&mut rt, window, 0xff09, Mods::NONE);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(2)));
    rt.step(steps()).unwrap();
    assert_eq!(
        damage.borrow().last(),
        Some(&DamageRegion::Rects(vec![plus, plus, minus, minus]))
    );
}

#[test]
fn a_click_updates_the_model_and_the_view_and_a_frame_with_no_damage_sends_nothing() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    let plus = rt.find(window, &Key::from("plus")).unwrap();
    assert_eq!(rt.node_at(window, Point::new(5.0, 5.0)), Some(plus));

    click(&mut rt, window, 5.0);

    assert_eq!(rt.model().count, 1);
    assert_eq!(
        rt.window_system().requests,
        vec![(window, WindowRequest::SetTitle("Count 1".into()))]
    );
    assert_eq!(
        rt.find(window, &Key::from("plus")),
        Some(plus),
        "the node survives the update"
    );
    assert_eq!(
        rt.window_system().presented.len(),
        1,
        "no frame callback yet"
    );

    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.window_system().presented.len(),
        1,
        "nothing visible changed, so no buffer is sent"
    );

    configure(&mut rt, window, 2);
    assert_eq!(
        rt.window_system().presented[1],
        (window, Some(ConfigureId(2)))
    );
}

#[test]
fn tab_moves_a_visible_focus_and_a_shortcut_sends_its_message() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);

    key(&mut rt, window, 0xff09, Mods::NONE);
    let plus = rt.find(window, &Key::from("plus")).unwrap();
    let snapshot = rt.snapshot(window).unwrap();
    assert_eq!(snapshot.focus, Some(plus));
    let (_, node) = snapshot
        .accessibility
        .iter()
        .find(|(id, _)| *id == plus)
        .unwrap();
    assert!(node.states.focused);
    assert_eq!(node.role, Role::Button);

    key(&mut rt, window, 0xfe20, Mods::SHIFT);
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &Key::from("minus")),
        "Shift+Tab wraps backwards"
    );

    key(&mut rt, window, 0x3d, Mods::PRIMARY);
    assert_eq!(rt.model().count, 1);
}

#[test]
fn a_task_reports_back_as_a_message_and_a_close_request_ends_the_application() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.messages.push_back("wait".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().done, vec![7]);
    assert_eq!(rt.pending().tasks, 0);

    rt.window_system_mut()
        .send(window, WindowEvent::CloseRequested);
    rt.step(steps()).unwrap();
    assert!(rt.exited());
}

#[test]
fn an_activation_token_reaches_the_update_that_asked_and_a_gone_window_gets_none() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.messages.push_back("launch".into());
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.window_system_mut().activations,
        vec![(window, Some("test.viewer".to_owned()))]
    );
    assert_eq!(rt.model().tokens, Vec::<Option<String>>::new());
    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::ActivationToken {
            request: ActivationRequest(1),
            token: Some("t-1".into()),
        });
    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::ActivationToken {
            request: ActivationRequest(9),
            token: Some("not asked for".into()),
        });
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().tokens, vec![Some("t-1".to_owned())]);

    rt.messages.push_back("launch from gone".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().tokens, vec![Some("t-1".to_owned()), None]);
    assert_eq!(rt.window_system_mut().activations.len(), 1);
}

/// A message waits for the window's first presented frame, however late its configure comes: not the commit alone, but
/// the frame callback that answers it, and a frame callback before any commit does not count. It comes once; asked for
/// once the window has presented, it comes at once; asked for a window the view does not have, it never comes.
#[test]
fn a_message_waits_until_the_window_has_presented_its_first_frame() {
    let (mut rt, window, _) = start();
    rt.messages.push_back("await main".into());
    rt.messages.push_back("await gone".into());
    rt.step(steps()).unwrap();
    assert!(rt.model().presented.is_empty(), "nothing is configured yet");
    rt.step(steps()).unwrap();
    assert!(rt.model().presented.is_empty(), "still nothing presented");
    assert!(rt.window_system().presented.is_empty());
    // a frame before anything was committed shows nothing
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.step(steps()).unwrap();
    assert!(rt.model().presented.is_empty(), "no frame was committed");

    configure(&mut rt, window, 1);
    assert_eq!(rt.window_system().presented.len(), 1);
    assert!(
        rt.model().presented.is_empty(),
        "committed, but the compositor has not answered the frame callback"
    );
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().presented, ["main"]);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().presented, ["main"], "once");

    rt.messages.push_back("await main".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().presented, ["main", "main"]);
    assert!(
        rt.on_presented.is_empty(),
        "the message for a window that never opened was dropped"
    );
}

/// A message asked for after a delay comes once the runtime's clock has passed it and not before, those due together in
/// the order they were asked for; the time of the next one bounds the runtime's wait.
#[test]
fn a_message_asked_for_after_a_delay_comes_once_the_clock_reaches_it() {
    let clock = Arc::new(FakeClock::default());
    let damage = Rc::default();
    let mut options = options(&damage);
    options.clock = clock.clone();
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    clock.0.store(1_000, Ordering::SeqCst);
    for message in ["later 500 b", "later 200 a", "later 500 c"] {
        rt.messages.push_back(message.into());
    }
    rt.step(steps()).unwrap();
    assert!(rt.model().presented.is_empty());
    assert_eq!(rt.pending().next_wake, Some(Timestamp(1_200)));
    clock.0.store(1_199, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert!(rt.model().presented.is_empty(), "not before its time");
    clock.0.store(1_200, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().presented, ["due a"]);
    assert_eq!(rt.pending().next_wake, Some(Timestamp(1_500)));
    clock.0.store(2_000, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().presented, ["due a", "due b", "due c"]);
    assert_eq!(rt.pending().next_wake, None);
}

/// The image is laid out at whole points and drawn at the window's scale rounded up to whole pixels per point, with the
/// hotspot where the update put it; the update hears each step of the drag until it ends, and nothing after.
#[test]
fn a_drag_draws_its_image_in_whole_points_and_pixels_and_hears_each_step_until_it_ends() {
    let (mut rt, window, _) = start();
    rt.window_system_mut().drag = true;
    configure(&mut rt, window, 1);
    rt.window_system_mut().send(window, WindowEvent::Scale(1.5));
    rt.messages.push_back("drag".into());
    rt.step(steps()).unwrap();
    let (from, request) = rt.window_system_mut().drags[0].clone();
    assert_eq!(from, window);
    assert_eq!(request.data[0].mime, "text/uri-list");
    assert_eq!(request.actions, [DropAction::Copy, DropAction::Move].into());
    let image = request.image.expect("an image");
    assert_eq!(
        (image.width, image.height, image.scale),
        (62, 40, 2),
        "31 × 20 points at 2 pixels a point"
    );
    assert_eq!(image.hotspot, Point::new(5.5, 4.0));

    let token = DragToken(1);
    for progress in [
        DragProgress::Target(Some("text/uri-list".into())),
        DragProgress::Action(Some(DropAction::Move)),
        DragProgress::Dropped,
        DragProgress::Finished,
        DragProgress::Cancelled,
    ] {
        rt.window_system_mut()
            .queue
            .push_back(PlatformEvent::Drag { token, progress });
    }
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.model().drags,
        [
            "Target(Some(\"text/uri-list\"))",
            "Action(Some(Move))",
            "Dropped",
            "Finished"
        ]
    );
}

/// A drag the platform refuses, or one from a window that is gone, is cancelled at once.
#[test]
fn a_drag_that_cannot_start_is_cancelled_at_once() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.messages.push_back("drag".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().drags, ["Cancelled"]);

    rt.window_system_mut().drag = true;
    rt.messages.push_back("drag from gone".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().drags, ["Cancelled", "Cancelled"]);
    assert!(rt.window_system_mut().drags.is_empty());
}

/// A window while `open`; closing it only hides it.
struct Resident;

impl Application for Resident {
    type Model = bool;
    type Message = String;
    const RESIDENT: bool = true;

    fn init() -> (bool, Vec<Command<String>>) {
        (true, Vec::new())
    }

    fn update(open: &mut bool, message: String) -> Vec<Command<String>> {
        match message.as_str() {
            "open" => *open = true,
            "close" => *open = false,
            "quit" => return vec![Command::Exit],
            _ => {}
        }
        Vec::new()
    }

    fn view(open: &bool) -> Vec<Window<String>> {
        let mut windows = Counter::view(&Model::default());
        windows[0].on_close = Some("close".into());
        windows.truncate(usize::from(*open));
        windows
    }
}

#[test]
fn a_runtime_started_from_a_model_shows_it_and_runs_its_commands_instead_of_init() {
    let damage = Rc::default();
    let mut rt = Runtime::<Resident, _>::start(
        FakeSystem::default(),
        options(&damage),
        (
            false,
            vec![Command::Perform(Box::pin(async { "open".to_owned() }))],
        ),
    );
    assert!(!*rt.model());
    rt.step(steps()).unwrap();
    assert!(*rt.model());
    assert_eq!(rt.windows().len(), 1);
}

#[test]
fn a_resident_application_outlives_its_last_window_until_it_exits() {
    let damage = Rc::default();
    let mut rt = Runtime::<Resident, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));

    rt.window_system_mut()
        .send(window, WindowEvent::CloseRequested);
    rt.step(steps()).unwrap();
    assert!(rt.windows().is_empty());
    assert!(!rt.exited());

    rt.messages.push_back("open".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.windows().len(), 1);
    assert!(!rt.exited());

    rt.messages.push_back("quit".into());
    rt.step(steps()).unwrap();
    assert!(rt.exited());
}

#[test]
fn the_snapshot_lists_the_nodes_in_reading_order_with_their_window_rects() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    let snapshot = rt.snapshot(window).unwrap();
    let rects: Vec<_> = snapshot.nodes.iter().map(|n| n.rect).collect();
    assert_eq!(
        rects,
        [
            cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0),
            cupertino_geometry::Rect::new(0.0, 0.0, 10.0, 10.0),
            cupertino_geometry::Rect::new(10.0, 0.0, 10.0, 10.0),
        ]
    );
    assert_eq!(snapshot.display_list.nodes.len(), 3);
    assert_eq!(
        snapshot.nodes[0].children,
        vec![snapshot.nodes[1].id, snapshot.nodes[2].id]
    );
}

#[test]
fn a_buffer_is_sized_as_the_window_system_rounds_a_fractional_scale() {
    assert_eq!(
        [3.0, 2.0, 1.0].map(|n| physical(n, 1.25)),
        [4, 3, 1],
        "3.75 and 2.5 round up, 1.25 down"
    );
    assert_eq!(physical(100.0, 1.5), 150);
}

#[test]
fn the_adapter_hears_a_windows_title_before_its_first_tree_and_when_it_changes() {
    let adapter = FakeAdapter::default();
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);
    adapter.active.set(true);
    rt.step(steps()).unwrap();
    rt.messages.push_back("plus".into());
    rt.step(steps()).unwrap();
    assert_eq!(
        *adapter.titles.borrow(),
        [
            (window, "Count 0".to_owned(), 0),
            (window, "Count 1".to_owned(), 1)
        ]
    );
}

#[test]
fn a_closed_window_is_dropped_by_the_accessibility_adapter() {
    let adapter = FakeAdapter::default();
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);
    rt.messages.push_back("close".into());
    rt.step(steps()).unwrap();
    assert_eq!(*adapter.closed.borrow(), [window]);
}

#[test]
fn an_assistive_technology_that_starts_after_the_app_gets_the_whole_tree_and_then_only_changes() {
    let adapter = FakeAdapter::default();
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);
    assert!(adapter.updates.borrow().is_empty());

    adapter.active.set(true);
    rt.step(steps()).unwrap();

    let snapshot = rt.snapshot(window).unwrap();
    assert_eq!(
        *adapter.updates.borrow(),
        [AccessibilityUpdate {
            window,
            root: snapshot.nodes[0].id,
            focus: None,
            nodes: snapshot.accessibility.clone(),
            removed: Vec::new(),
            full: true,
        }]
    );

    key(&mut rt, window, 0xff09, Mods::NONE);
    let plus = rt.find(window, &Key::from("plus")).unwrap();
    {
        let updates = adapter.updates.borrow();
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[1].focus, Some(plus));
        assert!(!updates[1].full);
        assert!(
            updates[1].nodes.len() < snapshot.accessibility.len(),
            "a diff, not the tree again"
        );
    }

    adapter.active.set(false);
    rt.step(steps()).unwrap();
    adapter.active.set(true);
    rt.step(steps()).unwrap();
    let updates = adapter.updates.borrow();
    assert_eq!(updates.len(), 3);
    assert!(
        updates[2].full,
        "an adapter back from inactive replaces its tree"
    );
    assert_eq!(updates[2].nodes, rt.snapshot(window).unwrap().accessibility);
}

#[test]
fn a_window_whose_content_was_too_large_to_draw_draws_again_on_its_next_change_without_a_configure()
{
    let damage = Rc::default();
    let broken = Rc::new(Cell::new(false));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&errors);
    let mut options = options_with(&damage, &broken);
    options.on_window_error = Some(Box::new(move |w, e| seen.borrow_mut().push((w, e.clone()))));
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure(&mut rt, window, 1);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();

    broken.set(true);
    key(&mut rt, window, 0xff09, Mods::NONE);
    assert_eq!(
        *errors.borrow(),
        [(
            window,
            UiError::Render(cupertino_ui_render::RenderError::ResourceLimit)
        )]
    );
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Idle,
        "no retry until something changes"
    );

    broken.set(false);
    key(&mut rt, window, 0xff09, Mods::NONE);
    assert_eq!(rt.window_system().presented.len(), 2);
    assert_eq!(damage.borrow().last(), Some(&DamageRegion::Full));
}

#[test]
fn a_window_the_renderer_cannot_draw_is_reported_and_draws_again_after_a_drawable_configure() {
    let errors = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&errors);
    let (mut rt, window, damage) = start_with(|o| {
        o.on_window_error = Some(Box::new(move |w, e| seen.borrow_mut().push((w, e.clone()))));
    });
    configure(&mut rt, window, 1);
    let side = (MAX_PIXELS as f64).sqrt() + 1.0;

    configure_at(&mut rt, window, 2, Size::new(side, side));

    assert!(!rt.exited());
    assert_eq!(
        *errors.borrow(),
        [(
            window,
            UiError::Render(cupertino_ui_render::RenderError::ResourceLimit)
        )]
    );
    assert_eq!(
        rt.step(steps()).unwrap(),
        StepStatus::Idle,
        "no retry until something changes"
    );
    key(&mut rt, window, 0xff09, Mods::NONE);
    assert_eq!(
        errors.borrow().len(),
        1,
        "a change tries again, and the same error is not reported again"
    );
    assert!(!rt.exited());

    configure(&mut rt, window, 3);
    assert_eq!(
        rt.window_system().presented,
        vec![
            (window, Some(ConfigureId(1))),
            (window, Some(ConfigureId(3)))
        ]
    );
    assert_eq!(damage.borrow().last(), Some(&DamageRegion::Full));
    assert_eq!(errors.borrow().len(), 1);
}

#[test]
fn a_protected_field_never_shows_its_text_to_the_input_method() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);

    key(&mut rt, window, 0xff09, Mods::NONE);
    key(&mut rt, window, 0xff09, Mods::NONE);

    let minus = rt.find(window, &Key::from("minus")).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().text_input, Some(minus));
    let sent = rt
        .window_system()
        .text_input
        .last()
        .cloned()
        .flatten()
        .unwrap();
    assert_eq!(sent.surrounding, None);
    assert_eq!(sent.purpose, cupertino_ui_text::ContentPurpose::Password);
}

#[test]
fn an_assistive_technology_listening_from_the_start_gets_each_window_once() {
    let adapter = FakeAdapter::default();
    adapter.active.set(true);
    let (mut rt, window, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    configure(&mut rt, window, 1);

    let updates = adapter.updates.borrow();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].nodes, rt.snapshot(window).unwrap().accessibility);
}

#[test]
fn the_adapter_learns_each_lock_state_the_window_system_reports() {
    let adapter = FakeAdapter::default();
    let (mut rt, _, _) = start_with(|o| o.accessibility = Some(Box::new(adapter.clone())));
    for locked in [false, true, false] {
        rt.window_system_mut()
            .queue
            .push_back(PlatformEvent::SessionLock(locked));
    }
    rt.step(steps()).unwrap();
    assert_eq!(*adapter.locked.borrow(), [false, true, false]);
}

#[test]
fn a_failing_window_reports_each_error_once_and_its_animation_waits_for_a_frame_that_presents() {
    let damage = Rc::default();
    let broken = Rc::new(Cell::new(false));
    let errors = Rc::new(RefCell::new(Vec::new()));
    let seen = Rc::clone(&errors);
    let mut options = options_with(&damage, &broken);
    options.on_window_error = Some(Box::new(move |w, e| seen.borrow_mut().push((w, e.clone()))));
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure(&mut rt, window, 1);
    key(&mut rt, window, 0xff09, Mods::NONE);
    key(&mut rt, window, 0xff09, Mods::NONE);
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    let presented = rt.window_system().presented.len();

    broken.set(true);
    key(&mut rt, window, 0x61, Mods::NONE);
    frame(&mut rt);
    assert_eq!(errors.borrow().len(), 1);
    let requested = rt.window_system().frames_requested;
    frame(&mut rt);
    frame(&mut rt);
    assert_eq!(errors.borrow().len(), 1, "the same error is reported once");
    assert_eq!(
        rt.window_system().frames_requested,
        requested,
        "the animation asks for no frames while the window fails"
    );

    broken.set(false);
    key(&mut rt, window, 0x61, Mods::NONE);
    assert_eq!(rt.window_system().presented.len(), presented + 1);
    assert!(rt.window_system().frames_requested > requested);
    broken.set(true);
    frame(&mut rt);
    assert_eq!(errors.borrow().len(), 2, "a failure after a success is new");
}

/// One node that takes every key, logging what reaches it.
struct Holder;

impl Application for Holder {
    type Model = Vec<String>;
    type Message = String;

    fn init() -> (Vec<String>, Vec<Command<String>>) {
        (Vec::new(), Vec::new())
    }

    fn update(log: &mut Vec<String>, message: String) -> Vec<Command<String>> {
        log.push(message);
        Vec::new()
    }

    fn view(_: &Vec<String>) -> Vec<Window<String>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Holder".into(),
                app_id: "test.holder".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Leaf {
                stop: true,
                ..leaf("hold")
            }),
            menus: None,
        }]
    }
}

#[test]
fn a_release_ends_its_press_by_physical_key_even_when_its_symbol_changed() {
    let damage = Rc::default();
    let mut rt = Runtime::<Holder, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let send = |rt: &mut Runtime<Holder, FakeSystem>, event| {
        rt.window_system_mut().send(window, event);
        rt.step(steps()).unwrap();
    };
    let key = |keysym, pressed| {
        WindowEvent::Key(
            KeyEvent {
                code: 30,
                keysym: Keysym(keysym),
                text: None,
                pressed,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        )
    };
    // `a` pressed, then Shift goes down and the same key is released as `A`
    send(&mut rt, key(0x61, true));
    send(&mut rt, key(0x41, false));
    send(&mut rt, WindowEvent::Focus(false));
    assert!(
        !rt.model().iter().any(|m| m.ends_with(":cancel")),
        "the released key holds nothing: {:?}",
        rt.model()
    );

    send(&mut rt, key(0x61, true));
    send(&mut rt, WindowEvent::Focus(true));
    send(&mut rt, WindowEvent::Focus(false));
    assert_eq!(
        rt.model().iter().filter(|m| m.ends_with(":cancel")).count(),
        1,
        "a key still held is cancelled"
    );
}

/// A dismissible node until it has heard Escape once, and beside it a field that logs its keys.
struct Dismisser;

impl Application for Dismisser {
    type Model = Vec<String>;
    type Message = String;

    fn init() -> (Vec<String>, Vec<Command<String>>) {
        (Vec::new(), Vec::new())
    }

    fn update(log: &mut Vec<String>, message: String) -> Vec<Command<String>> {
        log.push(message);
        Vec::new()
    }

    fn view(log: &Vec<String>) -> Vec<Window<String>> {
        let open = !log.iter().any(|m| m == "tip:Target:dismiss");
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Dismisser".into(),
                app_id: "test.dismisser".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(row(vec![
                Element::new(Leaf {
                    focusable: false,
                    stop: true,
                    dismissible: open,
                    ..leaf("tip")
                }),
                Element::new(leaf("field")),
            ])),
            menus: None,
        }]
    }
}

#[test]
fn a_deactivation_that_eats_a_dismissing_release_leaves_the_next_release_alone() {
    let damage = Rc::default();
    let mut rt = Runtime::<Dismisser, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let send = |rt: &mut Runtime<Dismisser, FakeSystem>, event| {
        rt.window_system_mut().send(window, event);
        rt.step(steps()).unwrap();
    };
    let key = |keysym, pressed| {
        WindowEvent::Key(
            KeyEvent {
                code: 1,
                keysym: Keysym(keysym),
                text: None,
                pressed,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        )
    };
    let (tab, escape) = (0xff09, 0xff1b);
    send(&mut rt, key(tab, true));
    send(&mut rt, key(escape, true));
    send(&mut rt, WindowEvent::Focus(false));
    send(&mut rt, WindowEvent::Focus(true));
    send(&mut rt, key(escape, true));
    send(&mut rt, key(escape, false));
    let field: Vec<_> = rt
        .model()
        .iter()
        .filter(|m| m.starts_with("field:") && m.ends_with(":key"))
        .collect();
    assert_eq!(
        field.len(),
        2,
        "the field hears the press and its release: {:?}",
        rt.model()
    );
    assert_eq!(
        rt.model()
            .iter()
            .filter(|m| *m == "tip:Target:dismiss")
            .count(),
        1
    );
}

#[test]
fn an_appearance_change_redraws_open_windows_with_the_new_theme_and_keeps_the_focus() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let (mut rt, window, _) = start();
    let dark = Arc::new(ThemeTokens {
        preferences: cupertino_ui_theme::ThemePreferences {
            appearance: cupertino_ui_theme::Appearance::Dark,
            reduce_transparency: false,
            reduce_motion: true,
        },
        ..(*rt.options.theme).clone()
    });
    let made = Arc::clone(&dark);
    let asked = Rc::clone(&seen);
    rt = rt.with_theme_for(Box::new(move |appearance: &SystemAppearance| {
        asked.borrow_mut().push(*appearance);
        Arc::clone(&made)
    }));
    configure(&mut rt, window, 1);
    key(&mut rt, window, 0xff09, Mods::NONE);
    let focus = rt.snapshot(window).unwrap().focus;
    assert!(focus.is_some());
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    let presented = rt.window_system().presented.len();

    let appearance = SystemAppearance {
        dark: true,
        reduce_motion: true,
        ..SystemAppearance::default()
    };
    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::Appearance(appearance));
    rt.step(steps()).unwrap();
    frame(&mut rt);

    assert_eq!(*seen.borrow(), [appearance]);
    assert!(
        Arc::ptr_eq(&rt.windows[0].env.theme, &dark),
        "the open window draws with it"
    );
    assert!(
        Arc::ptr_eq(&rt.options.theme, &dark),
        "a new window would too"
    );
    assert!(
        rt.window_system().presented.len() > presented,
        "the window draws again"
    );
    assert_eq!(rt.snapshot(window).unwrap().focus, focus);
}

#[test]
fn without_a_theme_maker_an_appearance_change_keeps_the_theme() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    let before = Arc::clone(&rt.windows[0].env.theme);
    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::Appearance(SystemAppearance {
            dark: true,
            ..SystemAppearance::default()
        }));
    rt.step(steps()).unwrap();
    assert!(Arc::ptr_eq(&rt.windows[0].env.theme, &before));
}

#[test]
fn a_theme_equal_to_the_current_one_lays_nothing_out_again() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert!(!rt.windows[0].dirty);
    let current = Arc::clone(&rt.options.theme);

    rt.set_theme(Arc::new((*current).clone()));
    assert!(!rt.windows[0].dirty, "an equal theme is no change");
    assert!(Arc::ptr_eq(&rt.windows[0].env.theme, &current));

    let mut other = (*current).clone();
    other.motion.reduce = !other.motion.reduce;
    rt.set_theme(Arc::new(other));
    assert!(
        rt.windows[0].dirty,
        "a different theme lays the window out again"
    );
}

/// One node that takes keys and asks to be woken 500 ns after each, logging what reaches it; `hide` unmounts it.
struct Sleeper;

impl Application for Sleeper {
    type Model = Vec<String>;
    type Message = String;

    fn init() -> (Vec<String>, Vec<Command<String>>) {
        (Vec::new(), Vec::new())
    }

    fn update(log: &mut Vec<String>, message: String) -> Vec<Command<String>> {
        log.push(message);
        Vec::new()
    }

    fn view(log: &Vec<String>) -> Vec<Window<String>> {
        let content = if log.iter().any(|m| m == "hide") {
            row(Vec::new())
        } else {
            row(vec![Element::new(Leaf {
                stop: true,
                wake_after: Some(500),
                ..leaf("sleeper")
            })])
        };
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Sleeper".into(),
                app_id: "test.sleeper".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(content),
            menus: None,
        }]
    }
}

type Sleepy = Runtime<Sleeper, FakeSystem>;

/// A sleeper focused by Tab, with a clock the test moves.
fn sleeper() -> (Sleepy, WindowId, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::default());
    let mut options = options(&Rc::default());
    options.clock = clock.clone();
    let mut rt = Sleepy::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    tap(&mut rt, window, 0xff09);
    (rt, window, clock)
}

fn tap(rt: &mut Sleepy, window: WindowId, keysym: u32) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
}

fn wakes(rt: &Sleepy) -> usize {
    rt.model().iter().filter(|m| m.ends_with(":wake")).count()
}

#[test]
fn a_node_is_woken_once_the_clock_reaches_the_time_it_asked_for() {
    let (mut rt, window, clock) = sleeper();
    clock.0.store(100, Ordering::SeqCst);
    tap(&mut rt, window, 0x61);
    assert_eq!(rt.pending().next_wake, Some(Timestamp(600)));

    clock.0.store(599, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(wakes(&rt), 0, "not before its time");

    clock.0.store(650, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(wakes(&rt), 1);
    assert_eq!(rt.pending().next_wake, None, "a wake is delivered once");
}

#[test]
fn a_new_request_moves_the_wake_and_an_unmounted_node_loses_it() {
    let (mut rt, window, clock) = sleeper();
    tap(&mut rt, window, 0x61);
    clock.0.store(300, Ordering::SeqCst);
    tap(&mut rt, window, 0x61);
    assert_eq!(rt.pending().next_wake, Some(Timestamp(800)));
    clock.0.store(700, Ordering::SeqCst);
    rt.step(steps()).unwrap();
    assert_eq!(wakes(&rt), 0, "the first request was replaced");

    rt.messages.push_back("hide".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.pending().next_wake, None);
}

#[test]
fn the_loop_waits_only_until_the_earliest_wake() {
    let (mut rt, window, clock) = sleeper();
    tap(&mut rt, window, 0x61);
    clock.0.store(200, Ordering::SeqCst);
    rt.window_system_mut().close_on_wait = true;
    let waits = Rc::clone(&rt.window_system().waits);
    rt.run().unwrap();
    // steps poll without waiting; the first wait that may block is `run`'s
    let blocking = waits
        .borrow()
        .iter()
        .copied()
        .find(|&w| w != Some(Duration::ZERO));
    assert_eq!(
        blocking,
        Some(Some(Duration::from_nanos(300))),
        "the wake due at 500 bounds the wait at 200: {:?}",
        waits.borrow()
    );
}

#[test]
fn a_theme_switch_repaints_what_draws_in_the_themes_colours() {
    use cupertino_ui_render::{Paint, PaintOp};
    use cupertino_ui_theme::{Appearance, ColorRole, ColorTokens, StateVariant};
    let themed = |appearance| {
        move |theme: &ThemeTokens| {
            Arc::new(ThemeTokens {
                colors: ColorTokens::neutral(appearance),
                ..theme.clone()
            })
        }
    };
    let (mut rt, window, _) = start_with(|o| o.theme = themed(Appearance::Light)(&o.theme));
    configure(&mut rt, window, 1);
    let frame = |rt: &mut Rt| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    // the window's background, the first thing its root node paints
    let background = |rt: &Rt| match &rt.snapshot(window).unwrap().display_list.nodes[0].ops[0] {
        PaintOp::Fill {
            paint: Paint::Solid(color),
            ..
        } => *color,
        other => panic!("the root paints its background first, not {other:?}"),
    };
    let light = background(&rt);
    let dark = themed(Appearance::Dark)(&rt.options.theme);
    let expected = dark
        .colors
        .get(ColorRole::WindowBackground, StateVariant::NORMAL)
        .unwrap();
    assert_ne!(
        light, expected,
        "the two themes differ in the window background"
    );

    rt.set_theme(dark);
    frame(&mut rt);
    assert_eq!(
        background(&rt),
        expected,
        "the same-size root paints in the new theme"
    );
}

#[test]
fn a_window_paints_the_theme_background_unless_its_content_draws_its_own() {
    let background = Color::new(0.9, 0.9, 0.9, 1.0);
    let (mut rt, window, _) = start_with(|o| {
        let mut theme = (*o.theme).clone();
        theme.colors = ColorTokens::new([(
            (ColorRole::WindowBackground, StateVariant::NORMAL),
            background,
        )]);
        o.theme = Arc::new(theme);
    });
    configure(&mut rt, window, 1);
    let first_op = |rt: &Rt| {
        rt.snapshot(window).unwrap().display_list.nodes[0]
            .ops
            .first()
            .cloned()
    };
    let fill = Some(PaintOp::Fill {
        shape: Shape::Rect(cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0)),
        paint: Paint::Solid(background),
    });
    assert_eq!(first_op(&rt), fill);

    rt.messages.push_back("panel".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(first_op(&rt), None, "a panel paints its own surface");

    rt.messages.push_back("panel".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(
        first_op(&rt),
        fill,
        "the root repaints when the background changes back"
    );
}

/// The material is shaped with the window's next present. New corners change nothing the window paints while the
/// compositor draws the material, so the change itself must bring a present.
#[test]
fn reshaping_the_material_presents_the_window_again() {
    let (mut rt, window, _) = start();
    configure(&mut rt, window, 1);
    rt.window_system_mut().effects = true;
    let frame = |rt: &mut Rt, message: &str| {
        rt.messages.push_back(message.into());
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(2)));
        rt.step(steps()).unwrap();
        rt.window_system().presented.len()
    };
    let shown = frame(&mut rt, "corners");
    assert_eq!(
        frame(&mut rt, "corners"),
        shown,
        "nothing changed, nothing to present"
    );
    assert!(
        frame(&mut rt, "rounder corners") > shown,
        "the reshaped material goes with a present"
    );
    let radius = rt
        .window_system()
        .requests
        .iter()
        .rev()
        .find_map(|(_, r)| match r {
            WindowRequest::Backdrop(Some(b)) => Some(b.radius),
            _ => None,
        });
    assert_eq!(radius, Some(16.0));
}

#[test]
fn a_menu_window_shows_its_glass_where_the_desktop_draws_it_and_the_opaque_fill_in_its_shape_elsewhere(
) {
    let background = Color::new(0.9, 0.9, 0.9, 1.0);
    let (mut rt, window, _) = start_with(|o| {
        let mut theme = (*o.theme).clone();
        theme.colors = ColorTokens::new([(
            (ColorRole::WindowBackground, StateVariant::NORMAL),
            background,
        )]);
        o.theme = Arc::new(theme);
    });
    configure(&mut rt, window, 1);
    let frame = |rt: &mut Rt, message: Option<&str>| {
        if let Some(m) = message {
            rt.messages.push_back(m.into());
        }
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
        rt.snapshot(window).unwrap().display_list.nodes[0]
            .ops
            .first()
            .cloned()
    };
    let backdrops = |rt: &Rt| -> Vec<Option<Backdrop>> {
        rt.window_system()
            .requests
            .iter()
            .filter_map(|(_, r)| match r {
                WindowRequest::Backdrop(b) => Some(b.clone()),
                _ => None,
            })
            .collect()
    };
    let opaque = Some(PaintOp::Fill {
        shape: Shape::RoundedRect {
            rect: cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0),
            radius: 8.0,
        },
        paint: Paint::Solid(background),
    });

    assert_eq!(
        frame(&mut rt, Some("menu")),
        opaque,
        "without the desktop's effects"
    );
    assert_eq!(backdrops(&rt), Vec::new());
    assert_eq!(
        frame(&mut rt, Some("half")),
        Some(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: cupertino_geometry::Rect::new(0.0, 0.0, 200.0, 100.0),
                radius: 8.0,
            },
            paint: Paint::Solid(Color {
                a: 0.5,
                ..background
            }),
        }),
        "the fill fades with the background"
    );
    rt.messages.push_back("menu".into());

    rt.window_system_mut().effects = true;
    rt.messages.push_back("panel".into());
    rt.step(steps()).unwrap();
    assert_eq!(
        frame(&mut rt, Some("menu")),
        None,
        "the compositor draws the material"
    );
    let menu = Backdrop {
        material: cupertino_ui_theme::MENU_GLASS_MATERIAL.into(),
        radius: 8.0,
        alpha: 1.0,
    };
    assert_eq!(backdrops(&rt), vec![Some(menu.clone())]);
    frame(&mut rt, Some("half"));
    assert_eq!(
        backdrops(&rt).last(),
        Some(&Some(Backdrop {
            alpha: 0.5,
            ..menu.clone()
        })),
        "the material fades with the background"
    );
    frame(&mut rt, Some("nan"));
    assert_eq!(
        backdrops(&rt).last(),
        Some(&Some(menu.clone())),
        "an alpha that is not finite is opaque"
    );
    rt.messages.push_back("menu".into());

    let mut reduced = (*rt.options.theme).clone();
    reduced.preferences.reduce_transparency = true;
    rt.set_theme(Arc::new(reduced));
    assert_eq!(
        frame(&mut rt, None),
        None,
        "Reduce Transparency keeps the glass, which the compositor draws with its recipe for it"
    );
    assert_eq!(backdrops(&rt).last(), Some(&Some(menu)));
}

/// One of four choices in a roving group; only the selected one is a Tab stop, or every one with `every_stop`, as in a
/// menu. Arrows, Home and End choose another and move the focus to it.
struct Choice {
    index: usize,
    selected: bool,
    every_stop: bool,
}

const CHOICES: usize = 4;
/// A model that makes every choice a Tab stop and keeps them all so, as a menu's rows.
const EVERY_STOP: usize = usize::MAX;

impl Widget<String> for Choice {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(20.0),
            height: SizeSpec::Fixed(20.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.selected != old.selected {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(20.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        let Event::Key(key, _) = event else {
            return Propagation::Continue;
        };
        if !key.pressed || cx.node() != cx.target() {
            return Propagation::Continue;
        }
        let (target, index) = match key.keysym.0 {
            0xff53 => (FocusTarget::Next, (self.index + 1) % CHOICES),
            0xff51 => (FocusTarget::Previous, (self.index + CHOICES - 1) % CHOICES),
            0xff50 => (FocusTarget::First, 0),
            0xff57 => (FocusTarget::Last, CHOICES - 1),
            _ => return Propagation::Continue,
        };
        cx.emit(format!("choose:{index}"));
        cx.focus_item(target);
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::RadioButton);
        cx.roving_item();
        if self.selected || self.every_stop {
            cx.focusable();
        }
    }
}

/// A plain button, a roving group of four choices, and the model's selection; the model refuses choice 2.
struct Choices;

impl Application for Choices {
    type Model = usize;
    type Message = String;

    fn init() -> (usize, Vec<Command<String>>) {
        (0, Vec::new())
    }

    fn update(selected: &mut usize, message: String) -> Vec<Command<String>> {
        if let Some(key) = message.strip_prefix("move:") {
            return vec![Command::MoveFocus(Key::from(key.to_owned()))];
        }
        if message == "every" {
            *selected = EVERY_STOP;
            return vec![Command::Focus(Key::from("choice0"))];
        }
        if let Some(index) = message.strip_prefix("choose:") {
            let index: usize = index.parse().unwrap();
            if index != 2 && *selected != EVERY_STOP {
                *selected = index;
            }
        }
        Vec::new()
    }

    fn view(selected: &usize) -> Vec<Window<String>> {
        let choices = (0..CHOICES)
            .map(|index| {
                Element::new(Choice {
                    index,
                    selected: index == *selected,
                    every_stop: *selected == EVERY_STOP,
                })
                .key(Key::from(format!("choice{index}")))
            })
            .collect();
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Choices".into(),
                app_id: "test.choices".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(row(vec![
                Element::new(leaf("before")).key("before"),
                Element::new(Group {
                    roving: true,
                    ..row(choices)
                }),
            ])),
            menus: None,
        }]
    }
}

#[test]
fn arrows_move_the_focus_with_the_selection_in_a_roving_group_and_a_refused_one_keeps_it() {
    let damage = Rc::default();
    let mut rt = Runtime::<Choices, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let key = |rt: &mut Runtime<Choices, FakeSystem>, keysym: u32| {
        rt.window_system_mut().send(
            window,
            WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(keysym),
                    text: None,
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ),
        );
        rt.step(steps()).unwrap();
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    let focused = |rt: &Runtime<Choices, FakeSystem>| {
        let focus = rt.snapshot(window).unwrap().focus;
        (0..CHOICES).find(|i| focus == rt.find(window, &Key::from(format!("choice{i}"))))
    };

    key(&mut rt, 0xff09);
    key(&mut rt, 0xff09);
    assert_eq!(
        focused(&rt),
        Some(0),
        "Tab stops once in the group, on its selected item"
    );
    key(&mut rt, 0xff53);
    assert_eq!((focused(&rt), *rt.model()), (Some(1), 1));
    assert!(
        rt.windows[0].focus.visible(),
        "the ring shows on the moved focus"
    );

    key(&mut rt, 0xff53);
    assert_eq!(
        (focused(&rt), *rt.model()),
        (Some(1), 1),
        "a refused choice keeps the focus"
    );

    key(&mut rt, 0xff57);
    assert_eq!((focused(&rt), *rt.model()), (Some(3), 3));
    key(&mut rt, 0xff53);
    assert_eq!(
        (focused(&rt), *rt.model()),
        (Some(0), 0),
        "Next wraps to the first"
    );
    key(&mut rt, 0xff51);
    assert_eq!(focused(&rt), Some(3), "Previous wraps to the last");
}

#[test]
fn a_key_right_after_a_roving_move_goes_to_the_item_it_moved_to() {
    let damage = Rc::default();
    let mut rt = Runtime::<Choices, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    rt.messages.push_back("every".into());
    rt.step(steps()).unwrap();
    let right = WindowEvent::Key(
        KeyEvent {
            code: 0,
            keysym: Keysym(0xff53),
            text: None,
            pressed: true,
            repeat: false,
            mods: Mods::NONE,
        },
        Timestamp(0),
    );
    rt.window_system_mut().send(window, right.clone());
    rt.window_system_mut().send(window, right);
    rt.step(steps()).unwrap();
    let focus = rt.snapshot(window).unwrap().focus;
    assert_eq!(
        focus,
        rt.find(window, &Key::from("choice2")),
        "the second Right, before any frame, moves on from the first one's item"
    );
}

/// A vertical scroll container whose offset is the model's; it reports its geometry and reveals as messages.
struct Pane {
    name: &'static str,
    height: SizeSpec,
    offset: f64,
    content: Option<Element<String>>,
}

impl Widget<String> for Pane {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: self.height,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Scroll {
                horizontal: false,
                vertical: true,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<String>> {
        self.content.take().into_iter().collect()
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        match event {
            Event::ScrollGeometry { viewport, content } => cx.emit(format!(
                "geometry:{}:{}:{}",
                self.name, viewport.height, content.height
            )),
            Event::Reveal(offset) => cx.emit(format!("reveal:{}:{}", self.name, offset.y)),
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focus_fallback();
    }

    fn scroll_offset(&self, _: &()) -> Point {
        Point::new(0.0, self.offset)
    }
}

/// A block of a fixed height that a click focuses. An item of a roving group asks End to focus the item keyed `end`.
struct Block {
    height: f64,
    focusable: bool,
    end: Option<u64>,
}

fn block(height: f64, focusable: bool) -> Element<String> {
    Element::new(Block {
        height,
        focusable,
        end: None,
    })
}

impl Widget<String> for Block {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(50.0),
            height: SizeSpec::Fixed(self.height),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(50.0, self.height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) if self.focusable => {
                cx.request_focus();
            }
            Event::Key(key, _) if key.pressed && key.keysym.0 == 0xff57 => {
                let Some(end) = self.end else {
                    return Propagation::Continue;
                };
                cx.emit(format!("cursor:{end}"));
                cx.focus_item(FocusTarget::Key(Key::Int(end)));
            }
            _ => return Propagation::Continue,
        }
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.focusable {
            cx.role(Role::Button);
            cx.focusable();
        }
        if self.end.is_some() {
            cx.roving_item();
        }
    }
}

fn column(children: Vec<Element<String>>) -> Element<String> {
    struct Column(Vec<Element<String>>);
    impl Widget<String> for Column {
        type State = ();
        fn layout(&self) -> LayoutSpec {
            LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Start,
            })
        }
        fn take_children(&mut self) -> Vec<Element<String>> {
            std::mem::take(&mut self.0)
        }
        fn state(&self) {}
        fn update(&self, _: &Self, _: &mut ()) -> Change {
            Change::Layout
        }
        fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
            Measured {
                size: Size::new(0.0, 0.0),
                baseline: None,
            }
        }
        fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, String>) -> Propagation {
            Propagation::Continue
        }
        fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}
        fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
    }
    Element::new(Column(children))
}

/// An outer pane as tall as the window, scrolled 20 down: a focusable top block (30), a gap (200), an inner pane 40 tall around a
/// hidden field (100 + 20), one row of 20 per 20 of the outer viewport the model heard, and a roving list that builds
/// only its cursor item.
struct Scrolling;

struct Panes {
    outer: f64,
    inner: f64,
    viewport: f64,
    cursor: u64,
    heard: Vec<String>,
}

impl Application for Scrolling {
    type Model = Panes;
    type Message = String;

    fn init() -> (Panes, Vec<Command<String>>) {
        (
            Panes {
                outer: 20.0,
                inner: 0.0,
                viewport: 0.0,
                cursor: 0,
                heard: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Panes, message: String) -> Vec<Command<String>> {
        let parts: Vec<&str> = message.split(':').collect();
        match parts[..] {
            ["geometry", name, viewport, content] => {
                if name == "outer" {
                    model.viewport = viewport.parse().unwrap();
                }
                model.heard.push(format!("{name}:{viewport}:{content}"));
            }
            ["reveal", "outer", y] => model.outer = y.parse().unwrap(),
            ["reveal", "inner", y] => model.inner = y.parse().unwrap(),
            ["cursor", i] => model.cursor = i.parse().unwrap(),
            _ => {}
        }
        Vec::new()
    }

    fn view(model: &Panes) -> Vec<Window<String>> {
        let rows = (0..(model.viewport / 20.0) as usize).map(|_| block(20.0, false));
        let list = Element::new(Group {
            roving: true,
            ..group(vec![Element::new(Block {
                height: 20.0,
                focusable: true,
                end: Some(9),
            })
            .key(Key::Int(model.cursor))])
        });
        let content = column(
            [
                block(30.0, true).key("top"),
                block(200.0, false),
                Element::new(Pane {
                    name: "inner",
                    height: SizeSpec::Fixed(40.0),
                    offset: model.inner,
                    content: Some(column(vec![
                        block(100.0, false),
                        block(20.0, true).key("field"),
                    ])),
                }),
            ]
            .into_iter()
            .chain(rows)
            .chain([list])
            .collect(),
        );
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Scrolling".into(),
                app_id: "test.scrolling".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Pane {
                name: "outer",
                height: SizeSpec::Fill,
                offset: model.outer,
                content: Some(content),
            }),
            menus: None,
        }]
    }
}

fn scrolling_at(height: f64) -> (Runtime<Scrolling, FakeSystem>, WindowId) {
    let damage = Rc::default();
    let mut rt = Runtime::<Scrolling, FakeSystem>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, height));
    (rt, window)
}

fn scrolling() -> (Runtime<Scrolling, FakeSystem>, WindowId) {
    scrolling_at(100.0)
}

fn press(rt: &mut Runtime<Scrolling, FakeSystem>, window: WindowId, keysym: u32) {
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(keysym),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
}

fn rect_of(rt: &Runtime<Scrolling, FakeSystem>, window: WindowId, key: &'static str) -> Rect {
    let node = rt.find(window, &key.into()).unwrap();
    let snapshot = rt.snapshot(window).unwrap();
    snapshot.nodes.iter().find(|n| n.id == node).unwrap().rect
}

#[test]
fn the_model_hears_a_scroll_geometry_before_the_first_frame_paints() {
    let (rt, window) = scrolling_at(150.0);
    assert_eq!(
        rt.model().heard,
        [
            "outer:100:290",
            "inner:40:120",
            "outer:100:390",
            "outer:150:390",
            "outer:150:430"
        ],
        "the configured size, then the rows for it, reach the model"
    );
    assert_eq!(
        rt.window_system()
            .presented
            .iter()
            .filter(|(w, _)| *w == window)
            .count(),
        1,
        "only the frame with the rows for the viewport paints"
    );
    assert_eq!(
        rt.snapshot(window).unwrap().pending.frames,
        0,
        "and nothing waits to paint after it"
    );
}

#[test]
fn keyboard_focus_reveals_a_node_through_nested_panes_and_a_click_does_not() {
    let (mut rt, window) = scrolling();
    click(&mut rt, window, 10.0);
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &"top".into())
    );
    assert_eq!(
        rt.model().outer,
        20.0,
        "a click scrolls nothing under the pointer"
    );

    press(&mut rt, window, 0xff09);
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &"field".into())
    );
    // the inner pane moves the field to its bottom edge; the outer pane shows the inner viewport's part of it
    assert_eq!((rt.model().inner, rt.model().outer), (80.0, 170.0));
    assert_eq!(rect_of(&rt, window, "field").origin.y, 80.0);
}

#[test]
fn a_roving_move_to_a_key_focuses_the_item_the_new_view_builds() {
    let (mut rt, window) = scrolling();
    for _ in 0..3 {
        press(&mut rt, window, 0xff09);
    }
    let first = rt.find(window, &Key::Int(0)).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().focus, Some(first));
    press(&mut rt, window, 0xff57);
    assert_eq!(rt.model().cursor, 9);
    let end = rt.find(window, &Key::Int(9));
    assert!(end.is_some());
    assert_eq!(rt.snapshot(window).unwrap().focus, end);
}

/// An outer pane around an inner pane that holds nothing focusable.
struct NestedPanes;

impl Application for NestedPanes {
    type Model = ();
    type Message = String;

    fn init() -> ((), Vec<Command<String>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: String) -> Vec<Command<String>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        let inner = Element::new(Pane {
            name: "inner",
            height: SizeSpec::Fixed(40.0),
            offset: 0.0,
            content: Some(block(100.0, false)),
        })
        .key("inner");
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Nested".into(),
                app_id: "test.nested".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Pane {
                name: "outer",
                height: SizeSpec::Fill,
                offset: 0.0,
                content: Some(column(vec![inner, block(200.0, false)])),
            })
            .key("outer"),
            menus: None,
        }]
    }
}

#[test]
fn an_inner_pane_that_became_a_tab_stop_keeps_the_outer_pane_from_being_one() {
    let damage = Rc::default();
    let mut rt = Runtime::<NestedPanes, FakeSystem>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let inner = rt.find(window, &"inner".into());
    for _ in 0..2 {
        rt.window_system_mut().send(
            window,
            WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(0xff09),
                    text: None,
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ),
        );
        rt.step(steps()).unwrap();
        assert_eq!(rt.snapshot(window).unwrap().focus, inner);
    }
}

/// A window whose File menu has Save, disabled until there is an edit; the bar gains a View menu, or goes.
struct Menued;

#[derive(Default)]
struct Edits {
    edited: bool,
    view: bool,
    hidden: bool,
    chosen: Vec<String>,
}

fn action(name: &str) -> ActionName {
    ActionName::new(cupertino_menu_model::Scope::Win, name).unwrap()
}

fn menu(title: &str, item: &str) -> cupertino_menu_model::Menu {
    use cupertino_menu_model::{Entry, Item, Menu};
    let entry = Entry::Item(Item::new(item, action(&item.to_lowercase()), None));
    Menu::new(title, vec![vec![entry]])
}

impl Application for Menued {
    type Model = Edits;
    type Message = String;

    fn init() -> (Edits, Vec<Command<String>>) {
        (Edits::default(), Vec::new())
    }

    fn update(edits: &mut Edits, message: String) -> Vec<Command<String>> {
        match message.as_str() {
            "edit" => edits.edited = true,
            "view" => edits.view = true,
            "hide" => edits.hidden = true,
            "again" => {}
            _ => edits.chosen.push(message),
        }
        Vec::new()
    }

    fn view(edits: &Edits) -> Vec<Window<String>> {
        let mut menus = vec![menu("File", "Save")];
        if edits.view {
            menus.push(menu("View", "Zoom"));
        }
        let menus = Menus {
            bar: cupertino_menu_model::MenuBar {
                app: cupertino_menu_model::Menu::new("Notes", Vec::new()),
                menus,
            },
            disabled: (!edits.edited)
                .then(|| action("save"))
                .into_iter()
                .collect(),
            on_activate: Box::new(|a: ActionName| a.full()),
        };
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Notes".into(),
                app_id: "test.notes".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            content: Element::new(leaf("body")),
            popup: None,
            menus: (!edits.hidden).then_some(menus),
        }]
    }
}

#[test]
fn a_toplevel_asks_for_its_menus_only_when_they_change_and_a_chosen_item_sends_its_message() {
    let mut rt = Runtime::<Menued, FakeSystem>::new(FakeSystem::default(), options(&Rc::default()));
    let step = |rt: &mut Runtime<Menued, FakeSystem>, message: &str| {
        rt.messages.push_back(message.into());
        rt.step(steps()).unwrap();
    };
    let asked = |rt: &Runtime<Menued, FakeSystem>| -> Vec<Option<(Vec<String>, Vec<String>)>> {
        rt.window_system()
            .requests
            .iter()
            .filter_map(|(_, r)| match r {
                WindowRequest::Menus(menus) => Some(menus.as_ref().map(|m| {
                    let titles = m.bar.menus.iter().map(|m| m.title.clone()).collect();
                    (titles, m.disabled.iter().map(ActionName::full).collect())
                })),
                _ => None,
            })
            .collect()
    };
    let menus = |titles: &[&str], disabled: &[&str]| {
        Some((
            titles.iter().map(|t| t.to_string()).collect(),
            disabled.iter().map(|d| d.to_string()).collect(),
        ))
    };
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    assert_eq!(asked(&rt), [menus(&["File"], &["win.save"])]);

    step(&mut rt, "again");
    assert_eq!(
        asked(&rt).len(),
        1,
        "the same menus are not asked for again"
    );
    step(&mut rt, "edit");
    step(&mut rt, "view");
    assert_eq!(
        asked(&rt)[1..],
        [menus(&["File"], &[]), menus(&["File", "View"], &[])]
    );

    rt.window_system_mut()
        .send(window, WindowEvent::MenuActivated(action("zoom")));
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().chosen, ["win.zoom"]);

    step(&mut rt, "hide");
    assert_eq!(asked(&rt).last(), Some(&None));
}

/// Three keyed items in a row over a drop target that takes a URI list: a drop on `a` moves into it (a copy with
/// Option held), one on `c` is refused there, and anywhere else lands on the whole target as a copy. Item `b` sits in
/// a target of its own that takes only images, which a URI list passes by.
struct Shelf;

#[derive(Default)]
struct ShelfModel {
    heard: Vec<String>,
    /// How many drops were finished as each drop was handed over.
    finished_at_drop: Vec<usize>,
}

const URI_LIST: &str = "text/uri-list";

fn shelf_place(over: &DropOver<'_>) -> Option<DropPlace> {
    let a = Key::from("a");
    match over.keys.first() {
        Some(key) if *key == a => Some(DropPlace {
            key: Some(a),
            action: if over.mods.contains(Mods::ALT) {
                DropAction::Copy
            } else {
                DropAction::Move
            },
        }),
        Some(key) if *key == Key::from("c") => None,
        _ => Some(DropPlace {
            key: None,
            action: DropAction::Copy,
        }),
    }
}

impl Application for Shelf {
    type Model = ShelfModel;
    type Message = String;

    fn init() -> (ShelfModel, Vec<Command<String>>) {
        (ShelfModel::default(), Vec::new())
    }

    fn update(model: &mut ShelfModel, message: String) -> Vec<Command<String>> {
        let mut commands = Vec::new();
        if message.starts_with("drop ") {
            let finished = crate::test_support::DROPS_FINISHED.with(std::cell::Cell::get);
            model.finished_at_drop.push(finished);
            // the data names what the shelf did with it, where it did not do what was agreed
            if message.ends_with(": copied") {
                commands.push(Command::FinishDrop(DropToken(7), Some(DropAction::Copy)));
            } else if message.ends_with(": kept") {
                commands.push(Command::FinishDrop(DropToken(7), None));
            } else if message.ends_with(": held") {
                commands.push(Command::HoldDrop(DropToken(7)));
            } else if message.ends_with(": release") {
                // the held drop ends in a later turn, with what was done with it
                commands.push(Command::FinishDrop(DropToken(7), Some(DropAction::Copy)));
            }
        }
        model.heard.push(message);
        commands
    }

    fn view(_: &ShelfModel) -> Vec<Window<String>> {
        let images = Element::from(DropTarget {
            content: Element::new(leaf("b")).key("b"),
            mimes: vec!["image/png".into()],
            actions: [DropAction::Copy].into(),
            place: Box::new(|_| {
                Some(DropPlace {
                    key: None,
                    action: DropAction::Copy,
                })
            }),
            on_hover: None,
            on_drop: Box::new(|_| "image dropped".to_string()),
            on_leave: None,
        });
        let items = Element::new(row(vec![
            Element::new(leaf("a")).key("a"),
            images,
            Element::new(leaf("c")).key("c"),
        ]));
        vec![Window {
            key: WindowKey("shelf".into()),
            spec: WindowSpec {
                title: "Shelf".into(),
                app_id: "test.shelf".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::Theme,
            menus: None,
            content: Element::from(DropTarget {
                content: items,
                mimes: vec!["text/plain".into(), URI_LIST.into()],
                actions: [DropAction::Copy, DropAction::Move].into(),
                place: Box::new(shelf_place),
                on_hover: Some(Box::new(|place| {
                    format!("hover {:?}", place.map(|p| (p.key, p.action)))
                })),
                on_drop: Box::new(|dropped| {
                    format!(
                        "drop {:?} {:?} at {:?}: {}",
                        dropped.place,
                        dropped.action,
                        dropped.position,
                        String::from_utf8_lossy(&dropped.data.bytes)
                    )
                }),
                on_leave: Some("left".into()),
            }),
        }]
    }
}

type ShelfRt = Runtime<Shelf, FakeSystem>;

fn shelf() -> (ShelfRt, WindowId) {
    let damage = Rc::default();
    let mut options = options(&damage);
    let mut theme = (*options.theme).clone();
    theme.colors = ColorTokens::new([(
        (ColorRole::SelectedControlFill, StateVariant::NORMAL),
        Color::new(0.0, 0.4, 1.0, 1.0),
    )]);
    options.theme = Arc::new(theme);
    let mut rt = ShelfRt::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    (rt, window)
}

fn drag(rt: &mut ShelfRt, window: WindowId, event: DropEvent) {
    rt.window_system_mut()
        .send(window, WindowEvent::Drop(event));
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
}

fn enter(rt: &mut ShelfRt, window: WindowId, at: Point, actions: &[DropAction]) {
    drag(
        rt,
        window,
        DropEvent::Enter {
            token: DropToken(7),
            position: at,
            mimes: vec!["text/html".into(), URI_LIST.into()],
            actions: actions.iter().copied().collect(),
            mods: Mods::NONE,
        },
    );
}

/// The drop highlights the shelf shows: the rect of each translucent rounded fill.
fn highlights(rt: &ShelfRt, window: WindowId) -> Vec<Rect> {
    let list = rt.snapshot(window).unwrap().display_list;
    list.nodes
        .iter()
        .flat_map(|node| {
            let (x, y) = (node.transform.0[4], node.transform.0[5]);
            node.ops.iter().filter_map(move |op| match op {
                PaintOp::Fill {
                    shape: Shape::RoundedRect { rect, .. },
                    paint: Paint::Solid(color),
                } if color.a < 0.5 => Some(rect.translate(x, y)),
                _ => None,
            })
        })
        .collect()
}

fn last_answer(rt: &ShelfRt) -> DropAnswer {
    rt.window_system().drop_answers.last().unwrap().1.clone()
}

fn uri_answer(preferred: DropAction) -> DropAnswer {
    DropAnswer {
        mime: Some(URI_LIST.into()),
        actions: [DropAction::Copy, DropAction::Move].into(),
        preferred: Some(preferred),
    }
}

#[test]
fn a_drag_over_a_target_is_answered_highlighted_and_heard_as_it_moves_between_places() {
    let (mut rt, window) = shelf();
    let both = [DropAction::Copy, DropAction::Move];
    enter(&mut rt, window, Point::new(5.0, 5.0), &both);
    assert_eq!(last_answer(&rt), uri_answer(DropAction::Move));
    assert_eq!(rt.model().heard, ["hover Some((Some(Str(\"a\")), Move))"]);
    assert_eq!(highlights(&rt, window), [Rect::new(0.0, 0.0, 10.0, 10.0)]);

    // the images target around `b` takes no URI list, so the shelf takes the drag there, as a whole
    let moved = |rt: &mut ShelfRt, x: f64, mods: Mods| {
        drag(
            rt,
            window,
            DropEvent::Motion {
                position: Point::new(x, 5.0),
                mods,
            },
        )
    };
    moved(&mut rt, 15.0, Mods::NONE);
    assert_eq!(last_answer(&rt), uri_answer(DropAction::Copy));
    assert_eq!(rt.model().heard.last().unwrap(), "hover Some((None, Copy))");
    assert_eq!(highlights(&rt, window), [Rect::new(0.0, 0.0, 200.0, 100.0)]);

    let answers = rt.window_system().drop_answers.len();
    let heard = rt.model().heard.len();
    moved(&mut rt, 50.0, Mods::NONE);
    assert_eq!(
        rt.window_system().drop_answers.len(),
        answers,
        "the same answer is not told again"
    );
    assert_eq!(rt.model().heard.len(), heard, "nor the same place");

    moved(&mut rt, 25.0, Mods::NONE);
    assert_eq!(last_answer(&rt), DropAnswer::default(), "refused over `c`");
    assert_eq!(rt.model().heard.last().unwrap(), "hover None");
    assert!(highlights(&rt, window).is_empty());

    moved(&mut rt, 5.0, Mods::ALT);
    assert_eq!(last_answer(&rt), uri_answer(DropAction::Copy));

    drag(&mut rt, window, DropEvent::Leave);
    assert!(rt
        .model()
        .heard
        .ends_with(&["hover None".into(), "left".into()]));
    assert!(highlights(&rt, window).is_empty());
    assert!(rt.window_system().drop_reads.is_empty());
    assert!(rt.window_system().drops_finished.is_empty());
}

#[test]
fn a_drop_is_read_in_the_targets_type_handed_to_it_and_finished_with_the_chosen_action() {
    let (mut rt, window) = shelf();
    rt.window_system_mut().drop_data = Some(ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"file:///x\r\n".to_vec(),
    });
    enter(
        &mut rt,
        window,
        Point::new(4.0, 6.0),
        &[DropAction::Copy, DropAction::Move],
    );
    drag(&mut rt, window, DropEvent::Action(Some(DropAction::Move)));
    drag(&mut rt, window, DropEvent::Drop);
    assert_eq!(
        rt.window_system().drop_reads,
        [(DropToken(7), URI_LIST.to_owned())]
    );
    assert_eq!(
        rt.model().heard[1..],
        [
            "hover None".to_string(),
            "drop Some(Str(\"a\")) Move at Point { x: 4.0, y: 6.0 }: file:///x\r\n".to_string()
        ]
    );
    assert_eq!(
        rt.window_system().drops_finished,
        [(DropToken(7), Some(DropAction::Move))]
    );
    // the source hears the drop is done only after the model has it
    assert_eq!(rt.model().finished_at_drop, [0]);
    assert!(highlights(&rt, window).is_empty());
}

#[test]
fn a_drop_finishes_with_what_the_update_says_it_did_rather_than_the_action_agreed() {
    let (mut rt, window) = shelf();
    let both = [DropAction::Copy, DropAction::Move];
    for (data, done) in [("copied", Some(DropAction::Copy)), ("kept", None)] {
        rt.window_system_mut().drop_data = Some(ClipboardData {
            mime: URI_LIST.into(),
            bytes: data.as_bytes().to_vec(),
        });
        enter(&mut rt, window, Point::new(4.0, 6.0), &both);
        drag(&mut rt, window, DropEvent::Action(Some(DropAction::Move)));
        drag(&mut rt, window, DropEvent::Drop);
        assert!(rt.model().heard.last().unwrap().contains(" Move at "));
        assert_eq!(
            rt.window_system().drops_finished.last(),
            Some(&(DropToken(7), done)),
            "{data}"
        );
    }
    assert_eq!(rt.window_system().drops_finished.len(), 2);
}

#[test]
fn a_held_drop_finishes_only_when_a_later_update_says_what_was_done() {
    let (mut rt, window) = shelf();
    let both = [DropAction::Copy, DropAction::Move];
    rt.window_system_mut().drop_data = Some(ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"held".to_vec(),
    });
    enter(&mut rt, window, Point::new(4.0, 6.0), &both);
    drag(&mut rt, window, DropEvent::Drop);
    assert!(rt.model().heard.last().unwrap().ends_with(": held"));
    assert!(
        rt.window_system().drops_finished.is_empty(),
        "the source waits"
    );

    // another drop, whose update ends the held one
    rt.window_system_mut().drop_data = Some(ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"release".to_vec(),
    });
    drag(
        &mut rt,
        window,
        DropEvent::Enter {
            token: DropToken(8),
            position: Point::new(25.0, 50.0),
            mimes: vec![URI_LIST.into()],
            actions: both.into_iter().collect(),
            mods: Mods::NONE,
        },
    );
    drag(&mut rt, window, DropEvent::Drop);
    assert_eq!(
        rt.window_system().drops_finished,
        [
            (DropToken(7), Some(DropAction::Copy)),
            (DropToken(8), Some(DropAction::Copy))
        ]
    );
    // a held drop finishes once
    rt.window_system_mut().drop_data = Some(ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"release".to_vec(),
    });
    drag(
        &mut rt,
        window,
        DropEvent::Enter {
            token: DropToken(9),
            position: Point::new(25.0, 50.0),
            mimes: vec![URI_LIST.into()],
            actions: both.into_iter().collect(),
            mods: Mods::NONE,
        },
    );
    drag(&mut rt, window, DropEvent::Drop);
    assert_eq!(
        rt.window_system().drops_finished[2..],
        [(DropToken(9), Some(DropAction::Copy))]
    );
}

#[test]
fn without_the_window_systems_word_a_drop_takes_an_action_the_source_allows() {
    let (mut rt, window) = shelf();
    rt.window_system_mut().drop_data = Some(ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"file:///y".to_vec(),
    });
    // the source copies only, as before drag actions
    enter(&mut rt, window, Point::new(5.0, 5.0), &[DropAction::Copy]);
    drag(&mut rt, window, DropEvent::Drop);
    assert!(rt
        .model()
        .heard
        .last()
        .unwrap()
        .starts_with("drop Some(Str(\"a\")) Copy"));
    assert_eq!(
        rt.window_system().drops_finished,
        [(DropToken(7), Some(DropAction::Copy))]
    );
}

#[test]
fn a_drop_where_nothing_takes_it_or_whose_data_fails_is_refused() {
    let (mut rt, window) = shelf();
    let both = [DropAction::Copy, DropAction::Move];
    enter(&mut rt, window, Point::new(25.0, 5.0), &both);
    drag(&mut rt, window, DropEvent::Drop);
    assert!(rt.window_system().drop_reads.is_empty());
    assert_eq!(rt.window_system().drops_finished, [(DropToken(7), None)]);
    assert_eq!(rt.model().heard, ["left"], "nothing took it: the drag left");

    // the read fails: no data
    enter(&mut rt, window, Point::new(5.0, 5.0), &both);
    drag(&mut rt, window, DropEvent::Drop);
    assert_eq!(rt.window_system().drop_reads.len(), 1);
    assert_eq!(
        rt.window_system().drops_finished,
        [(DropToken(7), None), (DropToken(7), None)]
    );
    assert!(!rt.model().heard.iter().any(|m| m.starts_with("drop")));
    let heard = &rt.model().heard;
    assert_eq!(heard.last().unwrap(), "left", "the data failed: {heard:?}");
    assert_eq!(
        heard.iter().filter(|m| *m == "left").count(),
        2,
        "{heard:?}"
    );
}

/// The targets that ask for it hear once that a drag left the window, wherever it was over, and nothing of a drop a
/// target took.
#[test]
fn a_drag_that_leaves_the_window_is_heard_once_and_a_drop_is_not_a_leave() {
    let (mut rt, window) = shelf();
    let both = [DropAction::Copy, DropAction::Move];
    // over `c`, which takes nothing: no hover, but the window was entered
    enter(&mut rt, window, Point::new(25.0, 5.0), &both);
    assert!(rt.model().heard.is_empty());
    drag(&mut rt, window, DropEvent::Leave);
    drag(&mut rt, window, DropEvent::Leave);
    assert_eq!(rt.model().heard, ["left"]);

    rt.window_system_mut().drop_data = Some(ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"file:///z".to_vec(),
    });
    enter(&mut rt, window, Point::new(5.0, 5.0), &both);
    drag(&mut rt, window, DropEvent::Drop);
    drag(&mut rt, window, DropEvent::Leave);
    assert!(rt.model().heard.last().unwrap().starts_with("drop "));
    assert_eq!(rt.model().heard.iter().filter(|m| *m == "left").count(), 1);
}

/// A row inside a container with no role, beside a second button; `drop` takes the row away.
struct Fallen;

#[derive(Default)]
struct FallenModel {
    dropped: bool,
    heard: Vec<String>,
}

impl Application for Fallen {
    type Model = FallenModel;
    type Message = String;

    fn init() -> (FallenModel, Vec<Command<String>>) {
        (FallenModel::default(), Vec::new())
    }

    fn update(model: &mut FallenModel, message: String) -> Vec<Command<String>> {
        if message == "drop" {
            model.dropped = true;
        } else {
            model.heard.push(message);
        }
        Vec::new()
    }

    fn view(model: &FallenModel) -> Vec<Window<String>> {
        let rows = if model.dropped {
            Vec::new()
        } else {
            vec![Element::new(leaf("row")).key("row")]
        };
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Fallen".into(),
                app_id: "test.fallen".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::Theme,
            content: Element::new(row(vec![
                Element::new(group(rows)).key("rows"),
                Element::new(leaf("other")).key("other"),
            ])),
            menus: None,
        }]
    }
}

#[test]
fn a_focus_that_falls_to_a_node_without_a_role_is_told_on_its_nearest_ancestor_and_keeps_the_keyboard(
) {
    let adapter = FakeAdapter::default();
    adapter.active.set(true);
    let damage = Rc::default();
    let mut options = options(&damage);
    options.accessibility = Some(Box::new(adapter.clone()));
    let mut rt = Runtime::<Fallen, FakeSystem>::new(FakeSystem::default(), options);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let tab = |rt: &mut Runtime<Fallen, FakeSystem>, keysym: u32| {
        rt.window_system_mut().send(
            window,
            WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(keysym),
                    text: None,
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ),
        );
        rt.step(steps()).unwrap();
    };
    tab(&mut rt, 0xff09);
    let row = rt.find(window, &Key::from("row")).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().focus, Some(row));
    assert_eq!(adapter.updates.borrow().last().unwrap().focus, Some(row));

    // the row goes away: the keyboard falls to its container, which assistive technology does not see
    rt.messages.push_back("drop".into());
    rt.step(steps()).unwrap();
    let rows = rt.find(window, &Key::from("rows")).unwrap();
    let snapshot = rt.snapshot(window).unwrap();
    let root = snapshot.nodes[0].id;
    assert_eq!(
        snapshot.focus,
        Some(rows),
        "the keyboard stays where it fell"
    );
    assert!(snapshot.accessibility.iter().all(|(id, _)| *id != rows));
    assert_eq!(snapshot.accessibility_focus, Some(root));
    let told = adapter.updates.borrow().last().unwrap().clone();
    assert_eq!(
        told.focus,
        Some(root),
        "the reader is told of a node it holds"
    );
    let window_node = &snapshot
        .accessibility
        .iter()
        .find(|(id, _)| *id == root)
        .unwrap()
        .1;
    assert!(window_node.states.focused && window_node.states.focusable);

    // keys still go to the container, and Tab still goes on from where the keyboard is
    let before = rt.model().heard.len();
    tab(&mut rt, 0x78);
    // the container is the key's target, as the node holding the keyboard
    assert!(rt.model().heard[before..].contains(&"group:Target".to_owned()));
    tab(&mut rt, 0xff09);
    let other = rt.find(window, &Key::from("other")).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().focus, Some(other));
    assert_eq!(adapter.updates.borrow().last().unwrap().focus, Some(other));
    let snapshot = rt.snapshot(window).unwrap();
    let window_node = &snapshot
        .accessibility
        .iter()
        .find(|(id, _)| *id == root)
        .unwrap()
        .1;
    assert!(!window_node.states.focused, "the window lets the focus go");
}

#[test]
fn a_moved_focus_keeps_the_ring_as_it_showed() {
    let damage = Rc::default();
    let mut rt = Runtime::<Choices, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let focused = |rt: &Runtime<Choices, FakeSystem>| {
        let focus = rt.snapshot(window).unwrap().focus;
        (0..CHOICES).find(|i| focus == rt.find(window, &Key::from(format!("choice{i}"))))
    };
    // every choice a stop, the first focused with its ring as Focus gives it
    rt.messages.push_back("every".into());
    rt.step(steps()).unwrap();
    assert_eq!(focused(&rt), Some(0));
    assert!(rt.windows[0].focus.visible());

    rt.messages.push_back("move:choice2".into());
    rt.step(steps()).unwrap();
    assert_eq!(focused(&rt), Some(2));
    assert!(rt.windows[0].focus.visible(), "a ring that showed shows on");

    // a click's focus shows no ring, and the focus moved after it shows none either
    let node = rt.find(window, &Key::from("choice2"));
    let w = &mut rt.windows[0];
    w.focus.set(&rt.tree, node, false);
    rt.messages.push_back("move:choice3".into());
    rt.step(steps()).unwrap();
    assert_eq!(focused(&rt), Some(3));
    assert!(!rt.windows[0].focus.visible(), "no ring after a click");
}

/// With a sink, a scroll event is followed from its timestamp through its handlers, the model update, the frame that
/// drew it and the compositor showing it; without one nothing is kept.
#[test]
fn a_scroll_is_timed_from_its_input_to_the_frame_the_compositor_showed() {
    use crate::ScrollTiming;
    let clock = Arc::new(FakeClock::default());
    let damage = Rc::default();
    let mut options = options(&damage);
    options.clock = clock.clone();
    let seen: Rc<RefCell<Vec<ScrollTiming>>> = Rc::default();
    let sink = Rc::clone(&seen);
    let mut rt = Rt::new(FakeSystem::default(), options)
        .with_scroll_timing(Box::new(move |t| sink.borrow_mut().push(t)));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure(&mut rt, window, 1);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();

    // the event at 100, handled at 150 with a click that changes the model in the same step
    clock.0.store(150, Ordering::SeqCst);
    rt.window_system_mut().send(
        window,
        WindowEvent::Pointer(
            PointerEvent::Scroll {
                position: Point::new(5.0, 5.0),
                delta: Point::new(0.0, 10.0),
                value120: None,
                source: cupertino_ui_platform::ScrollSource::Finger,
                stop: false,
                inverted: (false, false),
            },
            Timestamp(100),
        ),
    );
    click(&mut rt, window, 5.0);
    assert_eq!(rt.model().count, 1);
    // the click changes only the title, which draws nothing: the next configure draws the frame
    configure(&mut rt, window, 2);
    let frame = *rt.window_system().frame_ids.last().unwrap();
    assert_eq!(rt.window_system().frame_ids.len(), 2);
    assert!(seen.borrow().is_empty(), "not shown yet");

    rt.window_system_mut().send(
        window,
        WindowEvent::Presented {
            frame,
            time: Timestamp(400),
        },
    );
    rt.step(steps()).unwrap();
    let timing = seen.borrow()[0];
    assert_eq!(seen.borrow().len(), 1);
    assert_eq!(
        (timing.window, timing.input, timing.handled, timing.updated),
        (window, Timestamp(100), Timestamp(150), Some(Timestamp(150)))
    );
    assert_eq!(timing.submitted, Timestamp(150));
    assert_eq!(timing.presented, Some(Timestamp(400)));
    let latency = timing.latency();
    assert_eq!((latency.handled, latency.presented), (50, Some(300)));
}

/// With a sink, every turn that presents a frame is timed phase by phase and the window system times its copies;
/// a turn that presents nothing is not heard. Without one, nothing is timed.
#[test]
fn a_presented_frame_is_timed_phase_by_phase_and_nothing_is_timed_without_a_sink() {
    use crate::FrameTiming;
    let damage = Rc::default();
    let untimed = Rt::new(FakeSystem::default(), options(&damage));
    assert!(!untimed.times_frames());
    assert!(!untimed.window_system().timed_copies);

    let seen: Rc<RefCell<Vec<FrameTiming>>> = Rc::default();
    let sink = Rc::clone(&seen);
    let mut rt = Rt::new(FakeSystem::default(), options(&damage))
        .with_frame_timing(Box::new(move |t| sink.borrow_mut().push(*t)));
    assert!(rt.times_frames());
    assert!(rt.window_system().timed_copies);
    rt.step(steps()).unwrap();
    assert!(
        seen.borrow().is_empty(),
        "no frame before the first configure"
    );
    let window = rt.windows()[0].1;
    configure(&mut rt, window, 1);
    let presents = rt.window_system().frame_ids.len();
    assert_eq!(presents, 1);
    let heard = seen.borrow().clone();
    assert_eq!(heard.len(), 1, "{heard:?}");
    let t = heard[0];
    assert_eq!(t.frames, 1);
    assert_eq!(
        t.copy,
        Duration::from_nanos(1),
        "the copy the window system timed"
    );
    assert!(t.raster > Duration::ZERO && t.paint > Duration::ZERO && t.layout > Duration::ZERO);
    let phases = t.update + t.view + t.layout + t.paint + t.raster + t.copy + t.commit;
    assert!(phases <= t.total, "{t:?}");

    // a turn that draws nothing is not a frame
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(2)));
    rt.step(steps()).unwrap();
    assert_eq!(seen.borrow().len(), 1);
}

/// Two text fields side by side.
struct TwoFields;

impl Application for TwoFields {
    type Model = ();
    type Message = String;

    fn init() -> ((), Vec<Command<String>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: String) -> Vec<Command<String>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        let mut windows = Counter::view(&Model::default());
        windows[0].content = Element::new(row(vec![
            Element::new(Leaf {
                secret: true,
                ..leaf("first")
            })
            .key("first"),
            Element::new(Leaf {
                secret: true,
                ..leaf("second")
            })
            .key("second"),
        ]));
        windows
    }
}

#[test]
fn the_input_method_lets_go_of_one_field_before_the_next_takes_it() {
    let damage = Rc::default();
    let mut rt = Runtime::<TwoFields, _>::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let tab = |rt: &mut Runtime<TwoFields, FakeSystem>| {
        rt.window_system_mut().send(
            window,
            WindowEvent::Key(
                KeyEvent {
                    code: 0,
                    keysym: Keysym(0xff09),
                    text: None,
                    pressed: true,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ),
        );
        rt.step(steps()).unwrap();
    };
    tab(&mut rt);
    let first = rt.find(window, &Key::from("first")).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().text_input, Some(first));
    let before = rt.window_system().text_input.len();
    tab(&mut rt);
    let second = rt.find(window, &Key::from("second")).unwrap();
    assert_eq!(rt.snapshot(window).unwrap().text_input, Some(second));
    let told: Vec<bool> = rt.window_system().text_input[before..]
        .iter()
        .map(Option::is_some)
        .collect();
    assert_eq!(
        told,
        [false, true],
        "the first field's input method is turned off, so what it composed never reaches the second"
    );
}

/// Where the release of a press that opened menus lands, and what each menu hears of it.
mod popup_release {
    use super::*;
    use crate::test_support::spec;
    use crate::{Popup, PopupAnchor};
    use std::sync::Arc;

    /// The space between a menu's items, which hits the menu itself.
    const GAP: f64 = 10.0;

    /// A menu's container: its items side by side `GAP` apart. It tells each button event it hears: the phase, whether
    /// the point is inside it and the press that opened its popup.
    struct Menu {
        label: &'static str,
        children: Vec<Element<String>>,
        /// It draws its own glass and fades out as it closes.
        closes_as_menu: bool,
    }

    impl Widget<String> for Menu {
        type State = ();

        fn layout(&self) -> LayoutSpec {
            spec(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: GAP,
                align: Align::Start,
                justify: Justify::Start,
            })
        }

        fn take_children(&mut self) -> Vec<Element<String>> {
            std::mem::take(&mut self.children)
        }

        fn state(&self) {}

        fn update(&self, _: &Self, _: &mut ()) -> Change {
            Change::None
        }

        fn closes_as_menu(&self) -> bool {
            self.closes_as_menu
        }

        fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
            Measured {
                size: Size::new(0.0, 0.0),
                baseline: None,
            }
        }

        fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
            if let Event::Pointer(
                PointerEvent::Button {
                    position, pressed, ..
                },
                _,
            ) = event
            {
                let place = match cx.bounds().is_some_and(|b| b.contains(*position)) {
                    true => "inside",
                    false => "outside",
                };
                let what = if *pressed { "press" } else { "release" };
                let opened = cx.opening_press().map(|t| t.0);
                cx.emit(format!(
                    "{}:{:?}:{what} {place}, opened by {opened:?}",
                    self.label,
                    cx.phase()
                ));
            }
            Propagation::Continue
        }

        fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

        fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
    }

    /// A window with an opener that opens a menu as it is pressed, a second window, and a submenu opened by moving
    /// over the menu's `more` item.
    struct Menus;

    #[derive(Default)]
    struct Shown {
        menu: bool,
        submenu: bool,
        /// The menus draw their own glass, so they fade out as they close.
        own_glass: bool,
        /// Every message, in order.
        heard: Vec<String>,
    }

    fn toplevel(key: &str, content: Element<String>) -> Window<String> {
        Window {
            key: WindowKey(key.to_owned().into()),
            spec: WindowSpec {
                title: key.to_owned(),
                app_id: "test.menus".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::Theme,
            content,
            menus: None,
        }
    }

    fn popup(key: &str, parent: &str, at: Point, content: Element<String>) -> Window<String> {
        Window {
            popup: Some(Popup {
                parent: WindowKey(parent.to_owned().into()),
                anchor: PopupAnchor::At(at),
                grab: true,
                on_dismiss: format!("{key} dismissed"),
            }),
            spec: WindowSpec {
                size: Size::new(100.0, 40.0),
                ..toplevel(key, Element::new(leaf("unused"))).spec
            },
            ..toplevel(key, content)
        }
    }

    impl Application for Menus {
        type Model = Shown;
        type Message = String;

        fn init() -> (Shown, Vec<Command<String>>) {
            (Shown::default(), Vec::new())
        }

        fn update(model: &mut Shown, message: String) -> Vec<Command<String>> {
            match message.as_str() {
                "opener:Target:press" => model.menu = true,
                "more:Target:motion" => model.submenu = true,
                "other:Target:press" => model.own_glass = true,
                "menu dismissed" => model.menu = false,
                _ => {}
            }
            model.heard.push(message);
            Vec::new()
        }

        fn view(model: &Shown) -> Vec<Window<String>> {
            let mut windows = vec![
                toplevel(
                    "main",
                    Element::new(row(vec![
                        Element::new(leaf("opener")).key("opener"),
                        Element::new(leaf("other")),
                    ])),
                ),
                toplevel(
                    "second",
                    Element::new(row(vec![Element::new(leaf("elsewhere"))])),
                ),
            ];
            if model.menu {
                windows.push(popup(
                    "menu",
                    "main",
                    Point::new(0.0, 10.0),
                    Element::new(Menu {
                        label: "menu",
                        children: vec![Element::new(leaf("item")), Element::new(leaf("more"))],
                        closes_as_menu: model.own_glass,
                    }),
                ));
            }
            if model.submenu {
                windows.push(popup(
                    "submenu",
                    "menu",
                    Point::new(30.0, 0.0),
                    Element::new(Menu {
                        label: "submenu",
                        children: vec![Element::new(leaf("deep"))],
                        closes_as_menu: model.own_glass,
                    }),
                ));
            }
            windows
        }
    }

    type Rt = Runtime<Menus, FakeSystem>;

    /// Points of the windows: each item is 10 points square, the menus' items `GAP` apart from their left edge.
    const OPENER: Point = Point::new(5.0, 5.0);
    const BACKGROUND: Point = Point::new(100.0, 50.0);
    const ITEM: Point = Point::new(5.0, 5.0);
    const BETWEEN: Point = Point::new(15.0, 5.0);
    const MORE: Point = Point::new(25.0, 5.0);
    const DEEP: Point = Point::new(5.0, 5.0);

    /// The press that opens the menu comes at this time.
    const PRESSED: u64 = 1_000;

    struct Desk {
        rt: Rt,
        clock: Arc<FakeClock>,
        configures: u32,
    }

    impl Desk {
        /// The two windows shown, nothing open.
        fn new() -> Self {
            let clock = Arc::new(FakeClock::default());
            let mut options = options(&Rc::default());
            options.clock = clock.clone();
            let mut system = FakeSystem::default();
            system.popups = true;
            let mut rt = Rt::new(system, options);
            rt.step(steps()).unwrap();
            let mut desk = Self {
                rt,
                clock,
                configures: 0,
            };
            desk.show("main", Size::new(200.0, 100.0));
            desk.show("second", Size::new(200.0, 100.0));
            desk
        }

        fn id(&self, key: &str) -> WindowId {
            self.rt
                .windows()
                .into_iter()
                .find(|(k, _)| *k == WindowKey(key.to_owned().into()))
                .unwrap_or_else(|| panic!("no window {key}"))
                .1
        }

        fn show(&mut self, key: &str, size: Size) {
            self.configures += 1;
            let id = self.id(key);
            configure_at(&mut self.rt, id, self.configures, size);
        }

        fn at(&mut self, time: u64) {
            self.clock.0.store(time, Ordering::SeqCst);
        }

        fn send(&mut self, key: &str, event: PointerEvent) {
            let id = self.id(key);
            self.rt.window_system_mut().send(
                id,
                WindowEvent::Pointer(event, Timestamp(self.clock.now().0)),
            );
            self.rt.step(steps()).unwrap();
        }

        fn button(&mut self, key: &str, position: Point, pressed: bool) {
            self.send(
                key,
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: Mods::NONE,
                },
            );
        }

        fn motion(&mut self, key: &str, position: Point) {
            self.send(key, PointerEvent::Motion { position });
        }

        /// The opener pressed at `PRESSED` and held: the menu opens and is shown.
        fn press_opener(&mut self) {
            self.at(PRESSED);
            self.button("main", OPENER, true);
            self.show("menu", Size::new(100.0, 40.0));
        }

        /// The pointer, still held, moves over the menu's `more`: the submenu opens and is shown.
        fn open_submenu(&mut self) {
            self.motion("menu", MORE);
            assert!(
                self.rt.model().submenu,
                "moving over more opens the submenu"
            );
            self.show("submenu", Size::new(100.0, 40.0));
        }

        /// The button let go in window `key` at `position`, some time after the press; what was heard of it.
        fn release(&mut self, key: &str, position: Point) -> Vec<String> {
            let before = self.rt.model().heard.len();
            self.at(PRESSED + 500);
            self.button(key, position, false);
            self.rt.model().heard[before..].to_vec()
        }
    }

    /// What `menu` heard of a button.
    fn told<'a>(heard: &'a [String], menu: &str) -> Vec<&'a str> {
        let prefix = format!("{menu}:");
        heard
            .iter()
            .filter(|m| m.starts_with(&prefix))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn a_press_on_the_opener_dragged_off_the_menu_and_let_go_tells_the_menu_of_a_release_outside_it(
    ) {
        let mut desk = Desk::new();
        desk.press_opener();
        assert_eq!(
            desk.rt.window_system().popups_created.len(),
            1,
            "the press opens the menu"
        );
        desk.motion("menu", ITEM);
        desk.motion("main", BACKGROUND);
        let heard = desk.release("main", BACKGROUND);
        assert_eq!(
            told(&heard, "menu"),
            ["menu:Target:release outside, opened by Some(1000)"],
            "heard: {heard:?}"
        );
    }

    impl Desk {
        /// Window `key` gains or loses the keyboard.
        fn focus(&mut self, key: &str, focused: bool) {
            let id = self.id(key);
            self.rt
                .window_system_mut()
                .send(id, WindowEvent::Focus(focused));
            self.rt.step(steps()).unwrap();
        }

        /// Whether window `key` still repaints on the second of two frames at one time: its colours are crossing.
        fn crossing(&mut self, key: &str) -> bool {
            let id = self.id(key);
            let presents = |desk: &Self| {
                let system = desk.rt.window_system();
                system.presented.iter().filter(|(w, _)| *w == id).count()
            };
            let mut counts = Vec::new();
            for _ in 0..2 {
                let now = Timestamp(self.clock.now().0);
                self.rt
                    .window_system_mut()
                    .send(id, WindowEvent::Frame(now));
                self.rt.step(steps()).unwrap();
                counts.push(presents(self));
            }
            counts[1] > counts[0]
        }
    }

    impl Desk {
        /// How many frames window `key` presented so far.
        fn presents(&self, key: &str) -> usize {
            let id = self.id(key);
            let system = self.rt.window_system();
            system.presented.iter().filter(|(w, _)| *w == id).count()
        }

        /// The keyboard leaves window `from` for window `to`, both heard in one dispatch, as the compositor sends them.
        fn keyboard(&mut self, from: &str, to: &str) {
            let (from, to) = (self.id(from), self.id(to));
            let system = self.rt.window_system_mut();
            system.send(from, WindowEvent::Focus(false));
            system.send(to, WindowEvent::Focus(true));
            self.rt.step(steps()).unwrap();
        }

        /// Answers the frame callbacks of every window, as the compositor does after showing them.
        fn frames(&mut self) {
            let now = Timestamp(self.clock.now().0);
            for (_, id) in self.rt.windows() {
                self.rt
                    .window_system_mut()
                    .send(id, WindowEvent::Frame(now));
            }
            self.rt.step(steps()).unwrap();
        }
    }

    /// The window a menu opens from stays drawn active while its menu and then the menu's submenu take the keyboard,
    /// so it presents no frame for either: its pixels are as they were, and the menus' first frames wait for no
    /// repaint of it.
    #[test]
    fn a_menu_and_its_submenu_taking_the_keyboard_repaint_nothing_of_the_window_they_open_from() {
        let mut desk = Desk::new();
        desk.focus("main", true);
        desk.frames();
        desk.press_opener();
        desk.frames();
        let before = desk.presents("main");
        desk.keyboard("main", "menu");
        desk.frames();
        assert_eq!(desk.presents("main"), before, "the menu took the keyboard");
        desk.open_submenu();
        desk.keyboard("menu", "submenu");
        desk.frames();
        assert_eq!(
            desk.presents("main"),
            before,
            "the submenu took the keyboard"
        );
    }

    /// A window that the keyboard really leaves is painted again in its inactive colours, and once more as it comes
    /// back.
    #[test]
    fn the_keyboard_leaving_for_another_window_repaints_the_window_it_left() {
        let mut desk = Desk::new();
        desk.focus("main", true);
        desk.frames();
        let before = desk.presents("main");
        desk.keyboard("main", "second");
        desk.frames();
        assert_eq!(desk.presents("main"), before + 1, "drawn inactive");
        desk.keyboard("second", "main");
        desk.frames();
        assert!(desk.presents("main") > before + 1, "drawn active again");
    }

    /// A window whose menu has the keyboard is drawn active all along, so when the menu closes and the keyboard comes
    /// back its colours do not cross again; a window that was drawn inactive crosses them as the keyboard comes back.
    #[test]
    fn the_keyboard_back_from_a_closed_menu_crosses_no_colours_but_back_from_elsewhere_does() {
        let mut desk = Desk::new();
        desk.press_opener();
        desk.focus("main", false);
        desk.focus("menu", true);
        let menu = desk.id("menu");
        desk.rt
            .window_system_mut()
            .send(menu, WindowEvent::PopupDone);
        desk.rt.step(steps()).unwrap();
        assert!(!desk.rt.model().menu, "the menu closed");
        desk.focus("main", true);
        assert!(
            !desk.crossing("main"),
            "the window was drawn active while its menu had the keyboard"
        );

        desk.focus("main", false);
        desk.focus("main", true);
        assert!(desk.crossing("main"), "back from elsewhere, it crosses");
    }

    /// A menu that draws its own glass (one with a pointer) fades out as a menu on the menu surface does: its window
    /// stays for the fade, takes no input from its start, and goes once it is over; one that does not goes at once.
    #[test]
    fn a_menu_that_draws_its_own_glass_fades_out_and_takes_no_input_meanwhile() {
        for own_glass in [false, true] {
            let mut desk = Desk::new();
            if own_glass {
                // the main window's second item, right of the opener
                desk.button("main", Point::new(15.0, 5.0), true);
                desk.button("main", Point::new(15.0, 5.0), false);
                assert!(desk.rt.model().own_glass);
            }
            desk.press_opener();
            desk.frames();
            let menu = desk.id("menu");
            desk.rt
                .window_system_mut()
                .send(menu, WindowEvent::PopupDone);
            desk.rt.step(steps()).unwrap();
            assert!(!desk.rt.model().menu, "the menu closed");
            let system = desk.rt.window_system();
            assert_eq!(
                system.destroyed.contains(&menu),
                !own_glass,
                "own glass {own_glass}: destroyed at once only without it"
            );
            if !own_glass {
                continue;
            }
            assert!(
                system
                    .requests
                    .contains(&(menu, WindowRequest::SetInputRegion(Some(Vec::new())))),
                "out of the input as the fade begins"
            );
            desk.at(PRESSED + 200_000_000);
            desk.rt.step(steps()).unwrap();
            assert!(
                !desk.rt.window_system().destroyed.contains(&menu),
                "still fading at 0.2 s"
            );
            desk.at(PRESSED + 240_000_000);
            desk.rt.step(steps()).unwrap();
            assert!(
                desk.rt.window_system().destroyed.contains(&menu),
                "gone after the fade"
            );
        }
    }

    #[test]
    fn a_release_on_the_opener_tells_the_menu_of_a_release_outside_it() {
        let mut desk = Desk::new();
        desk.press_opener();
        let heard = desk.release("main", OPENER);
        assert_eq!(
            told(&heard, "menu"),
            ["menu:Target:release outside, opened by Some(1000)"],
            "heard: {heard:?}"
        );
        assert!(
            heard.iter().any(|m| m == "opener:Target:release"),
            "the opener hears its own release too: {heard:?}"
        );
    }

    #[test]
    fn a_release_in_another_window_tells_the_menu_of_a_release_outside_it() {
        let mut desk = Desk::new();
        desk.press_opener();
        let heard = desk.release("second", Point::new(5.0, 5.0));
        assert_eq!(
            told(&heard, "menu"),
            ["menu:Target:release outside, opened by Some(1000)"],
            "heard: {heard:?}"
        );
    }

    #[test]
    fn a_release_on_a_gap_between_items_reaches_the_menu_inside_it_with_the_press_that_opened_it() {
        let mut desk = Desk::new();
        desk.press_opener();
        desk.motion("menu", BETWEEN);
        let heard = desk.release("menu", BETWEEN);
        assert_eq!(
            told(&heard, "menu"),
            ["menu:Target:release inside, opened by Some(1000)"],
            "the menu itself is let go on, once and not as outside it: {heard:?}"
        );
        assert!(!heard.iter().any(|m| m.starts_with("item:")));
    }

    #[test]
    fn a_release_on_an_item_reaches_the_item_and_the_menu_inside_it() {
        let mut desk = Desk::new();
        desk.press_opener();
        desk.motion("menu", ITEM);
        let heard = desk.release("menu", ITEM);
        assert!(
            heard.iter().any(|m| m == "item:Target:release"),
            "heard: {heard:?}"
        );
        assert_eq!(
            told(&heard, "menu"),
            [
                "menu:Capture:release inside, opened by Some(1000)",
                "menu:Bubble:release inside, opened by Some(1000)"
            ],
            "heard: {heard:?}"
        );
    }

    #[test]
    fn nested_menus_opened_by_one_press_each_hear_a_release_outside_them_all() {
        let mut desk = Desk::new();
        desk.press_opener();
        desk.open_submenu();
        let heard = desk.release("main", BACKGROUND);
        assert_eq!(
            told(&heard, "menu"),
            ["menu:Target:release outside, opened by Some(1000)"],
            "heard: {heard:?}"
        );
        assert_eq!(
            told(&heard, "submenu"),
            ["submenu:Target:release outside, opened by Some(1000)"],
            "heard: {heard:?}"
        );
    }

    #[test]
    fn a_release_in_a_submenu_tells_neither_menu_of_a_release_outside() {
        let mut desk = Desk::new();
        desk.press_opener();
        desk.open_submenu();
        desk.motion("submenu", DEEP);
        let heard = desk.release("submenu", DEEP);
        assert!(
            heard.iter().any(|m| m == "deep:Target:release"),
            "heard: {heard:?}"
        );
        assert_eq!(
            told(&heard, "submenu"),
            [
                "submenu:Capture:release inside, opened by Some(1000)",
                "submenu:Bubble:release inside, opened by Some(1000)"
            ],
            "heard: {heard:?}"
        );
        assert_eq!(told(&heard, "menu"), Vec::<&str>::new(), "heard: {heard:?}");
    }

    #[test]
    fn a_release_on_a_gap_of_the_parent_menu_tells_the_submenu_nothing() {
        let mut desk = Desk::new();
        desk.press_opener();
        desk.open_submenu();
        desk.motion("menu", BETWEEN);
        let heard = desk.release("menu", BETWEEN);
        assert_eq!(
            told(&heard, "menu"),
            ["menu:Target:release inside, opened by Some(1000)"],
            "heard: {heard:?}"
        );
        assert_eq!(
            told(&heard, "submenu"),
            Vec::<&str>::new(),
            "heard: {heard:?}"
        );
    }

    /// A menu opened by a click, and a submenu opened after it with no button down: a later press is not theirs, so
    /// its release elsewhere tells them nothing, while each still knows the press that opened it, if any.
    #[test]
    fn a_later_press_let_go_elsewhere_tells_menus_it_did_not_open_nothing() {
        let mut desk = Desk::new();
        desk.press_opener();
        let clicked = desk.release("main", OPENER);
        assert_eq!(
            told(&clicked, "menu"),
            ["menu:Target:release outside, opened by Some(1000)"],
            "heard: {clicked:?}"
        );
        desk.open_submenu();

        desk.at(3_000);
        desk.button("menu", ITEM, true);
        desk.motion("main", BACKGROUND);
        let before = desk.rt.model().heard.len();
        desk.at(3_500);
        desk.button("main", BACKGROUND, false);
        let heard = desk.rt.model().heard[before..].to_vec();
        assert_eq!(told(&heard, "menu"), Vec::<&str>::new(), "heard: {heard:?}");
        assert_eq!(
            told(&heard, "submenu"),
            Vec::<&str>::new(),
            "heard: {heard:?}"
        );

        desk.at(4_000);
        desk.button("submenu", DEEP, true);
        let heard = desk.release("submenu", DEEP);
        assert_eq!(
            told(&heard, "submenu"),
            [
                "submenu:Capture:release inside, opened by None",
                "submenu:Bubble:release inside, opened by None"
            ],
            "the submenu opened with no button down: {heard:?}"
        );
        desk.button("menu", ITEM, true);
        let heard = desk.release("menu", ITEM);
        assert_eq!(
            told(&heard, "menu"),
            [
                "menu:Capture:release inside, opened by Some(1000)",
                "menu:Bubble:release inside, opened by Some(1000)"
            ],
            "heard: {heard:?}"
        );
    }

    /// The same menus followed by a finger. Every event of a finger goes to the window it went down in; a menu opened
    /// while it is held hears it move and lift at its own point, at its root.
    mod touch {
        use super::*;
        use cupertino_ui_platform::TouchEvent;

        /// What a finger did, as a word.
        fn what(event: &TouchEvent) -> &'static str {
            match event {
                TouchEvent::Down { .. } => "down",
                TouchEvent::Motion { .. } => "motion",
                TouchEvent::Up { .. } => "up",
                TouchEvent::Cancel => "cancel",
            }
        }

        /// A node that tells each touch event it hears and each cancel: the phase and, for a menu (one with
        /// children), whether the point is inside it and the press that opened its popup. A menu moved over at its
        /// `more` column tells that too.
        struct Finger {
            label: &'static str,
            /// A menu, which holds items, rather than an item.
            menu: bool,
            children: Vec<Element<String>>,
        }

        fn finger(label: &'static str, children: Vec<Element<String>>) -> Element<String> {
            Element::new(Finger {
                label,
                menu: !children.is_empty(),
                children,
            })
        }

        impl Widget<String> for Finger {
            type State = (Option<Point>, bool);

            fn layout(&self) -> LayoutSpec {
                if !self.menu {
                    spec(ContainerKind::Leaf)
                } else {
                    spec(ContainerKind::Stack {
                        axis: Axis::Horizontal,
                        spacing: GAP,
                        align: Align::Start,
                        justify: Justify::Start,
                    })
                }
            }

            fn take_children(&mut self) -> Vec<Element<String>> {
                std::mem::take(&mut self.children)
            }

            fn state(&self) -> (Option<Point>, bool) {
                (None, false)
            }

            fn update(&self, _: &Self, _: &mut (Option<Point>, bool)) -> Change {
                Change::None
            }

            fn measure(
                &self,
                _: &(Option<Point>, bool),
                _: Constraints,
                _: &mut MeasureCx<'_>,
            ) -> Measured {
                // a menu is as large as its items, an item 10 points square
                let side = if self.menu { 0.0 } else { 10.0 };
                Measured {
                    size: Size::new(side, side),
                    baseline: None,
                }
            }

            fn event(
                &self,
                state: &mut (Option<Point>, bool),
                event: &Event,
                cx: &mut EventCx<'_, String>,
            ) -> Propagation {
                let touch = match event {
                    Event::Touch(touch, _) => *touch,
                    Event::Cancel => {
                        cx.emit(format!("{}:{:?}:cancel", self.label, cx.phase()));
                        return Propagation::Continue;
                    }
                    _ => return Propagation::Continue,
                };
                if let TouchEvent::Down { position, .. } | TouchEvent::Motion { position, .. } =
                    touch
                {
                    state.0 = Some(position);
                }
                let mut message = format!("{}:{:?}:{}", self.label, cx.phase(), what(&touch));
                if self.menu {
                    let inside = state
                        .0
                        .is_some_and(|p| cx.bounds().is_some_and(|b| b.contains(p)));
                    let place = if inside { "inside" } else { "outside" };
                    let opened = cx.opening_press().map(|t| t.0);
                    message = format!("{message} {place}, opened by {opened:?}");
                    if let (TouchEvent::Motion { position, .. }, false) = (touch, state.1) {
                        if inside && position.x >= MORE.x - 5.0 && position.x < MORE.x + 5.0 {
                            state.1 = true;
                            cx.emit("more".to_owned());
                        }
                    }
                }
                cx.emit(message);
                Propagation::Continue
            }

            fn paint(&self, _: &(Option<Point>, bool), _: &mut PaintCx<'_>) {}

            fn semantics(&self, _: &(Option<Point>, bool), _: &mut SemanticsCx<'_>) {}
        }

        struct Touched;

        impl Application for Touched {
            type Model = Shown;
            type Message = String;

            fn init() -> (Shown, Vec<Command<String>>) {
                (Shown::default(), Vec::new())
            }

            fn update(model: &mut Shown, message: String) -> Vec<Command<String>> {
                match message.as_str() {
                    "opener:Target:down" => model.menu = true,
                    "more" => model.submenu = true,
                    _ => {}
                }
                model.heard.push(message);
                Vec::new()
            }

            fn view(model: &Shown) -> Vec<Window<String>> {
                let mut windows = vec![
                    toplevel(
                        "main",
                        Element::new(row(vec![
                            finger("opener", Vec::new()).key("opener"),
                            finger("other", Vec::new()),
                        ])),
                    ),
                    toplevel(
                        "second",
                        Element::new(row(vec![finger("elsewhere", Vec::new())])),
                    ),
                ];
                if model.menu {
                    windows.push(popup(
                        "menu",
                        "main",
                        MENU_AT,
                        finger(
                            "menu",
                            vec![finger("item", Vec::new()), finger("more", Vec::new())],
                        ),
                    ));
                }
                if model.submenu {
                    windows.push(popup(
                        "submenu",
                        "menu",
                        SUBMENU_AT,
                        finger("submenu", vec![finger("deep", Vec::new())]),
                    ));
                }
                windows
            }
        }

        /// Where the compositor places the menu in the main window and the submenu in the menu.
        const MENU_AT: Point = Point::new(0.0, 10.0);
        const SUBMENU_AT: Point = Point::new(30.0, 0.0);

        /// `p` in the menu, in the main window.
        fn in_menu(p: Point) -> Point {
            Point::new(p.x + MENU_AT.x, p.y + MENU_AT.y)
        }

        /// `p` in the submenu, in the main window.
        fn in_submenu(p: Point) -> Point {
            in_menu(Point::new(p.x + SUBMENU_AT.x, p.y + SUBMENU_AT.y))
        }

        /// The first finger.
        const ONE: i32 = 1;

        struct Hand {
            rt: Runtime<Touched, FakeSystem>,
            clock: Arc<FakeClock>,
            configures: u32,
        }

        impl Hand {
            /// The two windows shown, nothing open.
            fn new() -> Self {
                let clock = Arc::new(FakeClock::default());
                let mut options = options(&Rc::default());
                options.clock = clock.clone();
                let mut system = FakeSystem::default();
                system.popups = true;
                let mut rt = Runtime::new(system, options);
                rt.step(steps()).unwrap();
                let mut hand = Self {
                    rt,
                    clock,
                    configures: 0,
                };
                hand.show("main", Size::new(200.0, 100.0), None);
                hand.show("second", Size::new(200.0, 100.0), None);
                hand
            }

            fn id(&self, key: &str) -> WindowId {
                self.rt
                    .windows()
                    .into_iter()
                    .find(|(k, _)| *k == WindowKey(key.to_owned().into()))
                    .unwrap_or_else(|| panic!("no window {key}"))
                    .1
            }

            /// Window `key` is configured, a popup placed at `at` in its parent first.
            fn show(&mut self, key: &str, size: Size, at: Option<Point>) {
                let id = self.id(key);
                if let Some(at) = at {
                    self.rt.window_system_mut().send(
                        id,
                        WindowEvent::PopupPlaced(Rect::from_origin_size(at, size)),
                    );
                }
                self.configures += 1;
                configure_at(&mut self.rt, id, self.configures, size);
            }

            fn at(&mut self, time: u64) {
                self.clock.0.store(time, Ordering::SeqCst);
            }

            /// What was heard of `event` sent to window `key`.
            fn send(&mut self, key: &str, event: TouchEvent) -> Vec<String> {
                let before = self.rt.model().heard.len();
                let id = self.id(key);
                self.rt
                    .window_system_mut()
                    .send(id, WindowEvent::Touch(event, Timestamp(self.clock.now().0)));
                self.rt.step(steps()).unwrap();
                self.rt.model().heard[before..].to_vec()
            }

            fn down(&mut self, key: &str, id: i32, position: Point) -> Vec<String> {
                self.send(key, TouchEvent::Down { id, position })
            }

            fn slide(&mut self, key: &str, id: i32, position: Point) -> Vec<String> {
                self.send(key, TouchEvent::Motion { id, position })
            }

            /// The first finger put down on the opener at `PRESSED` and held: the menu opens and is shown. What the
            /// opener heard.
            fn hold_opener(&mut self) -> Vec<String> {
                self.at(PRESSED);
                let heard = self.down("main", ONE, OPENER);
                assert!(self.rt.model().menu, "the finger opens the menu");
                self.show("menu", Size::new(100.0, 40.0), Some(MENU_AT));
                heard
            }

            /// The finger, still held, slides over the menu's `more`: the submenu opens and is shown.
            fn open_submenu(&mut self) {
                self.slide("main", ONE, in_menu(MORE));
                assert!(
                    self.rt.model().submenu,
                    "sliding over more opens the submenu"
                );
                self.show("submenu", Size::new(100.0, 40.0), Some(SUBMENU_AT));
            }

            /// The first finger lifted some time after it went down; what was heard of it.
            fn lift(&mut self) -> Vec<String> {
                self.at(PRESSED + 500);
                self.send("main", TouchEvent::Up { id: ONE })
            }
        }

        /// What `menu` heard of a lift or a cancel.
        fn ended<'a>(heard: &'a [String], menu: &str) -> Vec<&'a str> {
            let prefix = format!("{menu}:");
            heard
                .iter()
                .filter(|m| m.starts_with(&prefix) && (m.contains(":up") || m.contains(":cancel")))
                .map(String::as_str)
                .collect()
        }

        #[test]
        fn a_finger_from_the_opener_slid_off_the_menu_and_lifted_tells_the_menu_of_a_lift_outside_it(
        ) {
            let mut hand = Hand::new();
            hand.hold_opener();
            let over = hand.slide("main", ONE, in_menu(ITEM));
            assert!(
                over.iter()
                    .any(|m| m == "menu:Target:motion inside, opened by Some(1000)"),
                "the menu hears the finger move over it: {over:?}"
            );
            hand.slide("main", ONE, BACKGROUND);
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up outside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
        }

        #[test]
        fn a_tap_on_the_opener_tells_the_menu_of_a_lift_outside_it_and_the_opener_lets_go_first() {
            let mut hand = Hand::new();
            let opened = hand.hold_opener();
            assert!(
                opened.iter().any(|m| m == "opener:Target:cancel"),
                "the opener lets go of the finger as the menu takes it: {opened:?}"
            );
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up outside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert!(
                heard.iter().any(|m| m == "opener:Target:up"),
                "the lift goes where it lands too, to an opener already let go of: {heard:?}"
            );
        }

        #[test]
        fn a_finger_slid_beyond_its_window_and_lifted_tells_the_menu_of_a_lift_outside_it() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.slide("main", ONE, Point::new(-40.0, 300.0));
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up outside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert!(
                !heard.iter().any(|m| m.starts_with("elsewhere:")),
                "another window hears nothing: {heard:?}"
            );
        }

        #[test]
        fn a_lift_on_a_gap_between_items_reaches_the_menu_inside_it_with_the_finger_that_opened_it()
        {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.slide("main", ONE, in_menu(BETWEEN));
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up inside, opened by Some(1000)"],
                "the menu itself is lifted on, once: {heard:?}"
            );
            assert!(
                !heard.iter().any(|m| m.starts_with("other:")),
                "heard: {heard:?}"
            );
        }

        #[test]
        fn a_lift_on_an_item_reaches_the_menu_inside_it() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.slide("main", ONE, in_menu(ITEM));
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up inside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert!(
                !heard
                    .iter()
                    .any(|m| m.starts_with("opener:") && m.contains(":up")),
                "the opener under the finger's first point does not hear it lift: {heard:?}"
            );
        }

        #[test]
        fn nested_menus_opened_by_one_finger_each_hear_a_lift_outside_them_all() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.open_submenu();
            hand.slide("main", ONE, BACKGROUND);
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up outside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert_eq!(
                ended(&heard, "submenu"),
                ["submenu:Target:up outside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
        }

        #[test]
        fn a_lift_in_a_submenu_tells_the_menu_nothing() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.open_submenu();
            let over = hand.slide("main", ONE, in_submenu(DEEP));
            assert!(
                over.iter()
                    .any(|m| m == "submenu:Target:motion inside, opened by Some(1000)"),
                "the submenu hears the finger at its own point: {over:?}"
            );
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "submenu"),
                ["submenu:Target:up inside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert_eq!(
                ended(&heard, "menu"),
                Vec::<&str>::new(),
                "heard: {heard:?}"
            );
        }

        #[test]
        fn a_lift_on_a_gap_of_the_parent_menu_tells_the_submenu_nothing() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.open_submenu();
            hand.slide("main", ONE, in_menu(BETWEEN));
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up inside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert_eq!(
                ended(&heard, "submenu"),
                Vec::<&str>::new(),
                "heard: {heard:?}"
            );
        }

        /// A menu and its submenu opened by a finger lifted on the menu's gap, so they stay open: a later finger is
        /// theirs only where it goes down, and each still knows the finger that opened it.
        #[test]
        fn a_later_finger_lifted_elsewhere_tells_menus_it_did_not_open_nothing() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.open_submenu();
            hand.slide("main", ONE, in_menu(BETWEEN));
            hand.lift();

            // a second finger, put down on the menu: its every event goes to the menu's window
            hand.at(3_000);
            let down = hand.down("menu", 2, ITEM);
            assert!(down.iter().any(|m| m == "item:Target:down"), "{down:?}");
            hand.slide("menu", 2, Point::new(150.0, 80.0));
            hand.at(3_500);
            let heard = hand.send("menu", TouchEvent::Up { id: 2 });
            assert_eq!(
                ended(&heard, "submenu"),
                Vec::<&str>::new(),
                "the submenu the first finger opened hears nothing: {heard:?}"
            );
            assert_eq!(
                ended(&heard, "menu"),
                [
                    "menu:Capture:up outside, opened by Some(1000)",
                    "menu:Bubble:up outside, opened by Some(1000)"
                ],
                "the menu hears it lift as the window it went down in, around the item: {heard:?}"
            );

            hand.at(4_000);
            hand.down("submenu", 3, DEEP);
            let heard = hand.send("submenu", TouchEvent::Up { id: 3 });
            assert_eq!(
                ended(&heard, "submenu"),
                [
                    "submenu:Capture:up inside, opened by Some(1000)",
                    "submenu:Bubble:up inside, opened by Some(1000)"
                ],
                "heard: {heard:?}"
            );
            assert_eq!(
                ended(&heard, "menu"),
                Vec::<&str>::new(),
                "heard: {heard:?}"
            );
        }

        #[test]
        fn a_second_finger_put_down_on_a_menu_the_first_holds_goes_nowhere() {
            let mut hand = Hand::new();
            hand.hold_opener();
            let heard = hand.down("menu", 2, ITEM);
            assert_eq!(heard, Vec::<String>::new(), "nothing hears it go down");
            let heard = hand.send("menu", TouchEvent::Up { id: 2 });
            assert_eq!(heard, Vec::<String>::new(), "nor lift");
            hand.slide("main", ONE, in_menu(ITEM));
            let heard = hand.lift();
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:up inside, opened by Some(1000)"],
                "the first finger still lifts on the menu: {heard:?}"
            );
        }

        #[test]
        fn a_cancelled_touch_ends_it_in_every_menu_it_opened_and_a_later_lift_is_not_forwarded() {
            let mut hand = Hand::new();
            hand.hold_opener();
            hand.open_submenu();
            let heard = hand.send("main", TouchEvent::Cancel);
            assert_eq!(
                ended(&heard, "menu"),
                ["menu:Target:cancel inside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            assert_eq!(
                ended(&heard, "submenu"),
                ["submenu:Target:cancel outside, opened by Some(1000)"],
                "heard: {heard:?}"
            );
            let heard = hand.lift();
            assert!(
                ended(&heard, "menu").is_empty() && ended(&heard, "submenu").is_empty(),
                "a finger cancelled is no longer followed: {heard:?}"
            );
        }
    }
}

thread_local! {
    /// What the last paint of a [`Marked`] node read of [`PaintCx::obscured`].
    static OBSCURED_SEEN: Cell<Option<cupertino_geometry::Insets>> = const { Cell::new(None) };
    /// What the last paint of a [`RaisedRow`] read of [`PaintCx::in_raised_layer`].
    static IN_LAYER_SEEN: Cell<Option<bool>> = const { Cell::new(None) };
    /// The [`RaisedRow`] floats over the window and covers nothing ([`Widget::obscures`]).
    static ROW_FLOATS: Cell<bool> = const { Cell::new(false) };
}

/// A row in the window's raised layer, 30 pt tall: glass under it where the window system draws it, else a fill of
/// its own, and a mark over it.
struct RaisedRow;

const ROW_FILL: Color = Color::new(0.5, 0.5, 0.5, 1.0);
const ROW_MARK: Color = Color::new(1.0, 0.0, 0.0, 1.0);

impl Widget<String> for RaisedRow {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(30.0),
            align_self: Some(Align::Start),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 30.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, String>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        IN_LAYER_SEEN.set(Some(cx.in_raised_layer()));
        let size = cx.size();
        if !cx.glass(crate::Glass::regular()) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(ROW_FILL),
            });
        }
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(10.0, 10.0, 10.0, 10.0)),
            paint: Paint::Solid(ROW_MARK),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn raised(&self, _: &()) -> bool {
        true
    }

    fn obscures(&self, _: &()) -> bool {
        !ROW_FLOATS.get()
    }
}

/// A block 20 pt tall that draws an overlay over itself, as a caret does, and notes how far raised nodes cover it.
struct Marked;

impl Widget<String> for Marked {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(50.0),
            height: SizeSpec::Fixed(20.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(50.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, String>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        OBSCURED_SEEN.set(Some(cx.obscured()));
        cx.overlay();
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn overlay(&self, _: &(), cx: &mut crate::OverlayCx<'_>) {
        cx.fill_pixels(
            Rect::new(0.0, 0.0, 2.0, 20.0),
            Color::new(0.0, 0.0, 1.0, 1.0),
        );
    }
}

/// Layers its children over each other, filling the window.
struct Layers(Vec<Element<String>>);

impl Widget<String> for Layers {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<String>> {
        std::mem::take(&mut self.0)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, String>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A page that scrolls under a raised row: the page's offset is the model; it starts with a focusable block under the
/// row and a marked block below it.
struct UnderRow;

impl Application for UnderRow {
    /// The page's offset, and whether the window shows one colour instead of its content.
    type Model = (f64, bool);
    type Message = String;

    fn init() -> ((f64, bool), Vec<Command<String>>) {
        ((20.0, false), Vec::new())
    }

    fn update((offset, solid): &mut (f64, bool), message: String) -> Vec<Command<String>> {
        let parts: Vec<&str> = message.split(':').collect();
        match parts[..] {
            ["reveal", "page", y] | ["scroll", y] => *offset = y.parse().unwrap(),
            ["solid", on] => *solid = on == "on",
            _ => {}
        }
        Vec::new()
    }

    fn view((offset, solid): &(f64, bool)) -> Vec<Window<String>> {
        let page = Element::new(Pane {
            name: "page",
            height: SizeSpec::Fill,
            offset: *offset,
            content: Some(column(vec![
                block(40.0, false),
                block(20.0, true).key("target"),
                Element::new(Marked).key("marked"),
                block(300.0, false),
            ])),
        })
        .key("page");
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Under".into(),
                app_id: "test.under".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: if *solid {
                WindowBackground::INPUT_ONLY
            } else {
                WindowBackground::Theme
            },
            popup: None,
            content: Element::new(Layers(vec![page, Element::new(RaisedRow).key("row")])),
            menus: None,
        }]
    }
}

type Under = Runtime<UnderRow, FakeSystem>;

/// A window under a raised row, on a window system that shows raised layers (`layers`) and the desktop's effects.
fn under_row(layers: bool) -> (Under, WindowId) {
    let damage = Rc::default();
    let mut system = FakeSystem::default();
    system.layers = layers;
    system.effects = true;
    let mut rt = Under::new(system, options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    (rt, window)
}

/// Whether the window's list (`layer`: its raised layer's) holds a node that fills with `color`.
fn draws(rt: &Under, layer: bool, color: Color) -> bool {
    let w = &rt.windows[0];
    let list = if layer {
        &w.layer.list
    } else {
        &w.display_list
    };
    list.nodes.iter().any(|n| {
        n.ops
            .iter()
            .any(|op| matches!(op, PaintOp::Fill { paint: Paint::Solid(c), .. } if *c == color))
    })
}

fn last_materials<A: Application>(rt: &Runtime<A, FakeSystem>) -> Vec<MaterialRegion> {
    rt.window_system()
        .requests
        .iter()
        .rev()
        .find_map(|(_, r)| match r {
            WindowRequest::Materials(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn a_raised_node_knows_it_draws_in_the_layer_and_the_runtime_shows_what_the_layer_drew() {
    for layers in [true, false] {
        IN_LAYER_SEEN.set(None);
        let (rt, window) = under_row(layers);
        assert_eq!(IN_LAYER_SEEN.get(), Some(layers), "layers {layers}");
        let list = rt.layer_display_list(window).unwrap();
        let marked = list.nodes.iter().any(|n| {
            n.ops.iter().any(
                |op| matches!(op, PaintOp::Fill { paint: Paint::Solid(c), .. } if *c == ROW_MARK),
            )
        });
        assert_eq!(
            marked, layers,
            "the row's mark in the layer's list (layers {layers})"
        );
    }
}

#[test]
fn a_raised_row_draws_in_its_own_layer_with_its_glass_above_the_content() {
    let (rt, _) = under_row(true);
    let presents = &rt.window_system().layer_presents;
    let (_, layer) = presents.last().expect("the layer was presented");
    let layer = layer.as_ref().expect("and shown");
    assert_eq!(
        layer.height, 30,
        "from the window's top down to the row's bottom"
    );
    assert_eq!(layer.size, (200, 30), "as wide as the window, at its scale");
    assert!(draws(&rt, true, ROW_MARK), "the row's mark is in the layer");
    assert!(!draws(&rt, false, ROW_MARK), "and not in the content");
    assert!(
        !draws(&rt, true, ROW_FILL) && !draws(&rt, false, ROW_FILL),
        "glass, no fill"
    );
    assert_eq!(
        last_materials(&rt),
        vec![MaterialRegion {
            material: cupertino_ui_theme::GLASS_MATERIAL.into(),
            rect: Rect::new(0.0, 0.0, 200.0, 30.0),
            radius: 0.0,
            alpha: 1.0,
            glass: Some(cupertino_ui_platform::GlassRegion {
                style: cupertino_ui_platform::GlassStyle::Regular,
                container: 0,
                spacing: 0.0,
            }),
            raised: true,
        }],
        "the row's glass lies above the content, under the layer"
    );
}

#[test]
fn content_scrolling_under_the_raised_row_presents_the_content_and_leaves_the_layer() {
    let (mut rt, window) = under_row(true);
    let layers = rt.window_system().layer_presents.len();
    let frames = rt.window_system().presented.len();
    rt.messages.push_back("scroll:35".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    assert_eq!(rt.model().0, 35.0);
    assert_eq!(
        rt.window_system().presented.len(),
        frames + 1,
        "the content moved"
    );
    assert_eq!(
        rt.window_system().layer_presents.len(),
        layers,
        "the layer, which did not change, is not drawn again: the scroll damages none of the header band"
    );
}

/// A raised row that comes first in the tree, over a pane filling the window with a material of its own.
struct RowFirst;

impl Application for RowFirst {
    type Model = ();
    type Message = String;

    fn init() -> ((), Vec<Command<String>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: String) -> Vec<Command<String>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        let mut window = UnderRow::view(&(0.0, false)).remove(0);
        window.content = Element::new(Layers(vec![
            Element::new(RaisedRow).key("row"),
            Element::new(GlassPane).key("pane"),
        ]));
        vec![window]
    }
}

/// Without a raised layer a raised node's material still lies over the materials of the nodes it lies over, though it
/// comes first in the tree (the order of the window's regions is the order they are drawn in); with a layer it lies
/// above the content anyway.
#[test]
fn without_a_layer_a_raised_row_s_glass_lies_over_a_material_that_comes_after_it_in_the_tree() {
    for layers in [false, true] {
        let damage = Rc::default();
        let mut system = FakeSystem::default();
        system.layers = layers;
        system.effects = true;
        let mut rt = Runtime::<RowFirst, FakeSystem>::new(system, options(&damage));
        rt.step(steps()).unwrap();
        let window = rt.windows()[0].1;
        configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
        let materials = rt
            .window_system()
            .requests
            .iter()
            .rev()
            .find_map(|(_, r)| match r {
                WindowRequest::Materials(m) => Some(m.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(materials.len(), 2, "{materials:?}");
        // the row is 30 points tall, the pane fills the window
        let row = materials
            .iter()
            .position(|m| m.rect.height() == 30.0)
            .unwrap();
        let pane = materials
            .iter()
            .position(|m| m.rect.height() == 100.0)
            .unwrap();
        if layers {
            assert!(materials[row].raised && !materials[pane].raised);
        } else {
            assert!(pane < row, "the row's glass is drawn last: {materials:?}");
            let w = &rt.windows[0];
            let order: Vec<_> = w.display_list.nodes.iter().map(|n| n.id).collect();
            let at = |key: &'static str| {
                let id: crate::NodeId = rt.find(w.id, &key.into()).unwrap();
                order.iter().position(|&n| n == id.into()).unwrap()
            };
            assert!(at("pane") < at("row"), "and its paint too");
        }
    }
}

/// A raised panel that floats over the window and covers nothing, as an alert does: glass where it can, a mark.
struct FloatingPanel;

impl Widget<String> for FloatingPanel {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        RaisedRow.layout()
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, state: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        RaisedRow.measure(state, constraints, cx)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, String>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.glass(crate::Glass::regular());
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            paint: Paint::Solid(ROW_FILL),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn raised(&self, _: &()) -> bool {
        true
    }

    fn obscures(&self, _: &()) -> bool {
        false
    }
}

/// A floating panel that comes first in the tree, then a raised row it lies across.
struct FloatFirst;

impl Application for FloatFirst {
    type Model = ();
    type Message = String;

    fn init() -> ((), Vec<Command<String>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: String) -> Vec<Command<String>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<String>> {
        let mut window = UnderRow::view(&(0.0, false)).remove(0);
        window.content = Element::new(Layers(vec![
            Element::new(FloatingPanel).key("panel"),
            Element::new(RaisedRow).key("row"),
        ]));
        vec![window]
    }
}

/// A raised node that floats draws over every other raised node, paint and glass, with or without a layer, wherever
/// it is in the tree: a bar's controls never lie over an alert.
#[test]
fn a_floating_raised_node_draws_over_the_other_raised_nodes_wherever_it_is_in_the_tree() {
    for layers in [false, true] {
        let damage = Rc::default();
        let mut system = FakeSystem::default();
        system.layers = layers;
        system.effects = true;
        let mut rt = Runtime::<FloatFirst, FakeSystem>::new(system, options(&damage));
        rt.step(steps()).unwrap();
        let window = rt.windows()[0].1;
        configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
        let materials = last_materials(&rt);
        assert_eq!(materials.len(), 2, "layers {layers}: {materials:?}");
        let w = &rt.windows[0];
        let position = |list: &crate::DisplayList, key: &'static str| {
            let id: crate::NodeId = rt.find(w.id, &key.into()).unwrap();
            list.nodes.iter().position(|n| n.id == id.into())
        };
        let panel: crate::NodeId = rt.find(w.id, &"panel".into()).unwrap();
        let panel_rect = w.geometry.get(&panel).unwrap().rect;
        let last = materials.last().unwrap();
        assert_eq!(
            last.rect, panel_rect,
            "layers {layers}: its glass is drawn last: {materials:?}"
        );
        if layers {
            // the row it lies across leaves the layer, whose paint lies over all its glass, for the content under
            // the panel's glass
            assert_eq!(position(&w.layer.list, "row"), None);
            assert!(position(&w.display_list, "row").is_some());
            assert!(position(&w.layer.list, "panel").is_some());
            assert!(last.raised && !materials[0].raised, "{materials:?}");
        } else {
            let list = &w.display_list;
            assert!(
                position(list, "row") < position(list, "panel"),
                "the panel paints last"
            );
        }
    }
}

/// A raised row a floating panel does not lie across keeps its place in the layer.
#[test]
fn a_raised_node_clear_of_a_floating_one_stays_in_the_layer() {
    struct Apart;
    impl Application for Apart {
        type Model = ();
        type Message = String;
        fn init() -> ((), Vec<Command<String>>) {
            ((), Vec::new())
        }
        fn update(_: &mut (), _: String) -> Vec<Command<String>> {
            Vec::new()
        }
        fn view(_: &()) -> Vec<Window<String>> {
            let mut window = UnderRow::view(&(0.0, false)).remove(0);
            window.content = Element::new(Layers(vec![
                Element::new(RaisedRow).key("row"),
                column(vec![
                    block(50.0, false),
                    Element::new(FloatingPanel).key("panel"),
                ]),
            ]));
            vec![window]
        }
    }
    let damage = Rc::default();
    let mut system = FakeSystem::default();
    system.layers = true;
    system.effects = true;
    let mut rt = Runtime::<Apart, FakeSystem>::new(system, options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    configure_at(&mut rt, window, 1, Size::new(200.0, 100.0));
    let materials = last_materials(&rt);
    assert_eq!(materials.len(), 2, "{materials:?}");
    assert!(materials.iter().all(|m| m.raised), "{materials:?}");
}

/// A window that turns one colour hides its raised layer with the colour's commit, as it takes its glass away, and
/// shows it again once its content is drawn again.
#[test]
fn a_window_turning_solid_hides_its_raised_layer_and_shows_it_again_when_drawn() {
    let (mut rt, window) = under_row(true);
    rt.window_system_mut().solid = true;
    let frame = |rt: &mut Under| {
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(1)));
        rt.step(steps()).unwrap();
    };
    frame(&mut rt);
    rt.messages.push_back("solid:on".into());
    rt.step(steps()).unwrap();
    assert_eq!(rt.window_system().solids.len(), 1);
    let presents = &rt.window_system().layer_presents;
    assert!(
        matches!(presents.last(), Some((w, None)) if *w == window),
        "the layer is hidden: {:?}",
        presents
            .iter()
            .map(|(_, l)| l.is_some())
            .collect::<Vec<_>>()
    );
    assert_eq!(rt.windows[0].layer.shown, None);
    let hidden = presents.len();

    frame(&mut rt);
    rt.messages.push_back("solid:off".into());
    rt.step(steps()).unwrap();
    let presents = &rt.window_system().layer_presents;
    assert_eq!(presents.len(), hidden + 1, "shown again with the content");
    assert!(matches!(presents.last(), Some((_, Some(l))) if l.height == 30));
}

#[test]
fn without_a_raised_layer_the_row_draws_in_place_and_nothing_scrolls_under_its_glass() {
    let (rt, _) = under_row(false);
    assert!(rt.window_system().layer_presents.is_empty());
    assert!(
        draws(&rt, false, ROW_MARK),
        "the row's mark is in the content"
    );
    assert!(!draws(&rt, false, ROW_FILL), "glass, no fill");
    let materials = last_materials(&rt);
    assert_eq!(materials.len(), 1, "{materials:?}");
    assert!(
        materials[0].glass.is_some() && !materials[0].raised,
        "the glass lies under the window, as any other: {materials:?}"
    );
    let w = &rt.windows[0];
    let order: Vec<_> = w.display_list.nodes.iter().map(|n| n.id).collect();
    let page = order
        .iter()
        .position(|&id| Some(id) == rt.find(w.id, &"page".into()).map(Into::into));
    let row = order
        .iter()
        .position(|&id| Some(id) == rt.find(w.id, &"row".into()).map(Into::into));
    assert!(page < row, "the row draws over the page");
    // the page clips what it scrolls to below the row, so none of it covers the glass
    let page: crate::NodeId = rt.find(w.id, &"page".into()).unwrap();
    let node = w
        .display_list
        .nodes
        .iter()
        .find(|n| n.id == page.into())
        .unwrap();
    assert_eq!(
        node.clip,
        Some(Shape::Rect(Rect::new(0.0, 30.0, 200.0, 70.0))),
        "cut at the row's bottom edge"
    );
    let target: crate::NodeId = rt.find(w.id, &"target".into()).unwrap();
    let target = w
        .display_list
        .nodes
        .iter()
        .find(|n| n.id == target.into())
        .unwrap();
    assert!(
        target.bounds.is_empty() || target.bounds.min_y() >= 29.0,
        "the block under the row is not drawn there: {:?}",
        target.bounds
    );
}

#[test]
fn a_raised_node_that_floats_draws_in_the_layer_but_obscures_nothing_under_it() {
    ROW_FLOATS.set(true);
    let (mut rt, window) = under_row(true);
    rt.messages.push_back("scroll:50".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    ROW_FLOATS.set(false);
    // the marked block lies at 10–30, all under the row, yet nothing covers it and its mark is drawn whole
    assert_eq!(OBSCURED_SEEN.get().map(|i| i.top), Some(0.0));
    assert_eq!(
        IN_LAYER_SEEN.get(),
        Some(true),
        "the row still draws in the layer"
    );
    let w = &rt.windows[0];
    let marked: crate::NodeId = rt.find(window, &"marked".into()).unwrap();
    let overlay = w
        .display_list
        .nodes
        .iter()
        .find(|n| n.id == crate::paint::overlay_id(marked))
        .expect("the mark");
    assert!(overlay.bounds.min_y() < 30.0, "{overlay:?}");
}

#[test]
fn focus_reveals_a_node_below_the_raised_row_and_overlays_are_cut_from_it() {
    let (mut rt, window) = under_row(true);
    // the page starts 20 pt down: the target (content 40–60) lies at 20–40, half under the 30 pt row
    assert_eq!(
        OBSCURED_SEEN.get().map(|i| i.top),
        Some(0.0),
        "the marked block, at 40–60, lies below the row"
    );
    rt.messages.push_back("scroll:50".into());
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    rt.step(steps()).unwrap();
    // the marked block (content 60–80) now lies at 10–30: all under the row
    assert_eq!(OBSCURED_SEEN.get().map(|i| i.top), Some(20.0));
    let w = &rt.windows[0];
    let marked: crate::NodeId = rt.find(window, &"marked".into()).unwrap();
    let overlay = w
        .display_list
        .nodes
        .iter()
        .find(|n| n.id == crate::paint::overlay_id(marked));
    assert!(
        overlay.is_none_or(|o| o.bounds.is_empty() || o.bounds.min_y() >= 30.0),
        "the mark is cut from the row: {overlay:?}"
    );
    // Tab to the target, at 40 - 50 = -10..10: the page shows it below the row, not at the window's top
    rt.window_system_mut().send(
        window,
        WindowEvent::Key(
            KeyEvent {
                code: 0,
                keysym: Keysym(0xff09),
                text: None,
                pressed: true,
                repeat: false,
                mods: Mods::NONE,
            },
            Timestamp(0),
        ),
    );
    rt.step(steps()).unwrap();
    assert_eq!(
        rt.snapshot(window).unwrap().focus,
        rt.find(window, &"target".into())
    );
    assert_eq!(rt.model().0, 10.0, "the target's top at the row's bottom");
}

/// A renderer that counts how many of its kind are alive, standing for the frame, layers and caches it keeps.
struct Counted {
    inner: Box<dyn UiRenderer<Frame = CpuFrame>>,
    alive: Rc<Cell<i32>>,
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.alive.set(self.alive.get() - 1);
    }
}

impl UiRenderer for Counted {
    type Frame = CpuFrame;

    fn render(
        &mut self,
        request: &RenderRequest<'_>,
    ) -> Result<cupertino_ui_render::RenderFrameId, RenderError> {
        self.inner.render(request)
    }

    fn frame(&self, id: cupertino_ui_render::RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.inner.frame(id)
    }
}

#[test]
fn a_change_of_renderer_frees_the_old_one_and_draws_whole_frames_with_a_new_one() {
    let (alive, made) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let (mut rt, window, damage) = start_with(|options| {
        let (alive, made) = (Rc::clone(&alive), Rc::clone(&made));
        let mut inner = std::mem::replace(&mut options.renderers, Box::new(|| unreachable!()));
        options.renderers = Box::new(move || {
            alive.set(alive.get() + 1);
            made.set(made.get() + 1);
            Box::new(Counted {
                inner: inner(),
                alive: Rc::clone(&alive),
            })
        });
    });
    configure(&mut rt, window, 1);
    assert_eq!((alive.get(), made.get()), (1, 1));
    damage.borrow_mut().clear();

    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::RendererChanged(
            cupertino_ui_platform::RendererKind::Gpu,
        ));
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.step(steps()).unwrap();
    assert_eq!(made.get(), 2, "a renderer of the new kind");
    assert_eq!(alive.get(), 1, "the old one is gone");
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
}

#[test]
fn lost_buffers_draw_a_whole_frame_of_a_window_that_changed_nothing() {
    let (mut rt, window, damage) = start();
    configure(&mut rt, window, 1);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.step(steps()).unwrap();
    damage.borrow_mut().clear();
    let presented = rt.window_system().presented.len();

    rt.window_system_mut()
        .queue
        .push_back(PlatformEvent::BuffersLost);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(0)));
    rt.step(steps()).unwrap();
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
    assert_eq!(rt.window_system().presented.len(), presented + 1);
}
