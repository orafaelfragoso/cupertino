//! What a frame repaints after a layout: a node that moved damages where it was and where it is, a layout that moves
//! nothing damages nothing, a group fading damages what its nodes draw and not the space between them, a configure
//! that changes nothing repaints nothing, a window the compositor keeps activated paints nothing again while another
//! surface has the keyboard, and a window drawn from damage alone is the window drawn whole, pixel for pixel.

use super::*;
use crate::test_support::{options, FakeSystem};
use crate::{Change, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget};
use cupertino_geometry::Rect;
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_platform::{Configure, DecorationMode, WindowStates, WmCapabilities};
use cupertino_ui_render::{CpuConfig, CpuRenderer, Paint, PaintOp, RenderFrameId, Shape};
use cupertino_ui_text::{GlyphKey, GlyphRasterizer, RasterGlyph, TextError};
use cupertino_ui_theme::Color;
use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;

const WINDOW: Size = Size::new(200.0, 100.0);

/// What the window shows: blocks at their places, on a board that may fill itself and fade as one group, and blank
/// spacers, which change the board's children without drawing anything.
#[derive(Clone, Debug, PartialEq)]
struct Scene {
    blocks: Vec<(u32, Rect, Color)>,
    fill: Option<Color>,
    opacity: f32,
    spacers: usize,
}

const RED: Color = Color::new(0.9, 0.1, 0.1, 1.0);
const BLUE: Color = Color::new(0.1, 0.2, 0.9, 0.8);
const GREY: Color = Color::new(0.5, 0.5, 0.5, 1.0);

impl Scene {
    fn two() -> Scene {
        Scene {
            blocks: vec![
                (1, Rect::new(10.0, 10.0, 20.0, 20.0), RED),
                (2, Rect::new(150.0, 60.0, 30.0, 20.0), BLUE),
            ],
            fill: None,
            opacity: 1.0,
            spacers: 0,
        }
    }
}

struct Boards;

impl Application for Boards {
    type Model = Scene;
    type Message = Scene;

    fn init() -> (Scene, Vec<Command<Scene>>) {
        (Scene::two(), Vec::new())
    }

    fn update(model: &mut Scene, message: Scene) -> Vec<Command<Scene>> {
        *model = message;
        Vec::new()
    }

    fn view(model: &Scene) -> Vec<Window<Scene>> {
        let mut children: Vec<Element<Scene>> = model
            .blocks
            .iter()
            .map(|&(key, rect, color)| {
                Element::new(Block { rect, color }).key(format!("block-{key}"))
            })
            .collect();
        children.extend((0..model.spacers).map(|i| {
            Element::new(Block {
                rect: Rect::new(0.0, 0.0, 0.0, 0.0),
                color: Color::new(0.0, 0.0, 0.0, 0.0),
            })
            .key(format!("spacer-{i}"))
        }));
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Boards".into(),
                app_id: "test.boards".into(),
                size: WINDOW,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::None,
            content: Element::new(Board {
                fill: model.fill,
                opacity: model.opacity,
                children,
            }),
            menus: None,
        }]
    }
}

/// Fills the window and places its children where they say; fills itself with `fill` and composites everything it
/// draws at `opacity`.
struct Board {
    fill: Option<Color>,
    opacity: f32,
    children: Vec<Element<Scene>>,
}

impl Widget<Scene> for Board {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Scene>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.fill != old.fill || self.opacity != old.opacity {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Scene>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        if let Some(color) = self.fill {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
        if self.opacity < 1.0 {
            cx.set_group(self.opacity, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A rectangle of one colour at `rect`; a clear one draws nothing.
struct Block {
    rect: Rect,
    color: Color,
}

impl Widget<Scene> for Block {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.size.width),
            height: SizeSpec::Fixed(self.rect.size.height),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.color != old.color {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Scene>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.color.a > 0.0 {
            let size = cx.size();
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: 3.0,
                },
                paint: Paint::Solid(self.color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

type Rt = Runtime<Boards, FakeSystem>;

fn steps() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}

fn configure(rt: &mut Rt, window: WindowId, id: u32, size: Size) {
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

/// The window drawn once, at `scale`, with the damage of every request it renders.
fn start(scale: f64) -> (Rt, WindowId, Rc<RefCell<Vec<DamageRegion>>>) {
    let damage = Rc::default();
    let mut rt = Rt::new(FakeSystem::default(), options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    if scale != 1.0 {
        rt.window_system_mut()
            .send(window, WindowEvent::Scale(scale));
    }
    configure(&mut rt, window, 1, WINDOW);
    assert_eq!(damage.borrow().as_slice(), [DamageRegion::Full]);
    (rt, window, damage)
}

/// The window shows `scene` at the next frame the compositor asks for.
fn show(rt: &mut Rt, window: WindowId, scene: Scene, time: u64) {
    rt.messages.push_back(scene);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(time)));
    rt.step(steps()).unwrap();
}

#[test]
fn moving_one_node_damages_only_where_it_was_and_where_it_is() {
    let (mut rt, window, damage) = start(1.0);
    let mut scene = Scene::two();
    scene.blocks[0].1 = Rect::new(60.0, 30.0, 20.0, 20.0);
    show(&mut rt, window, scene, 1);
    assert_eq!(
        damage.borrow().last(),
        Some(&DamageRegion::Rects(vec![
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Rect::new(60.0, 30.0, 20.0, 20.0)
        ]))
    );
}

#[test]
fn a_layout_that_moves_nothing_damages_nothing_and_presents_nothing() {
    let (mut rt, window, damage) = start(1.0);
    // the board fills itself, so a repaint of it would damage the whole window
    let filled = Scene {
        fill: Some(GREY),
        ..Scene::two()
    };
    show(&mut rt, window, filled.clone(), 1);
    let (rendered, presented) = (damage.borrow().len(), rt.window_system().presented.len());

    // new children that draw nothing: the board lays out again and every node keeps its place and its paint
    show(
        &mut rt,
        window,
        Scene {
            spacers: 3,
            ..filled.clone()
        },
        2,
    );
    assert_eq!(damage.borrow().len(), rendered, "nothing rendered");
    assert_eq!(
        rt.window_system().presented.len(),
        presented,
        "nothing presented"
    );

    // and a child going away and another coming in its place, at the same box
    let mut swapped = filled;
    swapped.blocks[0].0 = 9;
    show(&mut rt, window, swapped, 3);
    assert_eq!(
        damage.borrow().last(),
        Some(&DamageRegion::Rects(vec![
            Rect::new(10.0, 10.0, 20.0, 20.0),
            Rect::new(10.0, 10.0, 20.0, 20.0)
        ])),
        "the new block where it draws, the old one where it drew; the board not at all"
    );
}

#[test]
fn a_group_that_fades_damages_what_its_nodes_draw_and_not_the_space_between() {
    let (mut rt, window, damage) = start(1.0);
    let blocks = [
        Rect::new(10.0, 10.0, 20.0, 20.0),
        Rect::new(150.0, 60.0, 30.0, 20.0),
    ];
    for (time, opacity) in [(1, 0.5), (2, 0.25), (3, 1.0)] {
        show(
            &mut rt,
            window,
            Scene {
                opacity,
                ..Scene::two()
            },
            time,
        );
        assert_eq!(
            damage.borrow().last(),
            Some(&DamageRegion::Rects(blocks.to_vec())),
            "at opacity {opacity}"
        );
    }
}

#[test]
fn a_configure_that_changes_nothing_repaints_nothing_and_is_acknowledged() {
    let (mut rt, window, damage) = start(2.0);
    configure(&mut rt, window, 2, WINDOW);
    assert_eq!(
        damage.borrow().as_slice(),
        [DamageRegion::Full, DamageRegion::Rects(Vec::new())],
        "a commit that damages nothing"
    );
    assert_eq!(
        rt.window_system().presented.last(),
        Some(&(window, Some(ConfigureId(2))))
    );
    assert!(rt.windows[0].laid_out.is_some(), "the layout kept");

    // another size is laid out and drawn whole
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(1)));
    configure(&mut rt, window, 3, Size::new(220.0, 100.0));
    assert_eq!(damage.borrow().last(), Some(&DamageRegion::Full));
}

#[test]
fn a_window_kept_activated_while_a_desktop_surface_has_the_keyboard_paints_nothing_again() {
    let (mut rt, window, damage) = start(1.0);
    rt.window_system_mut().activated = Some(true);
    for (time, focused) in [(1, false), (2, true)] {
        rt.window_system_mut()
            .send(window, WindowEvent::Focus(focused));
        rt.window_system_mut()
            .send(window, WindowEvent::Frame(Timestamp(time)));
        rt.step(steps()).unwrap();
        assert!(rt.windows[0].active, "still the window in use");
        assert_eq!(damage.borrow().len(), 1, "nothing rendered");
    }

    // where the compositor says nothing of it, the keyboard leaving makes the window inactive, drawn whole
    rt.window_system_mut().activated = None;
    rt.window_system_mut()
        .send(window, WindowEvent::Focus(false));
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(3)));
    rt.step(steps()).unwrap();
    assert!(!rt.windows[0].active);
    assert_eq!(damage.borrow().last(), Some(&DamageRegion::Full));
}

/// Glyphs have no pixels: these windows draw no text.
struct NoText;

impl GlyphRasterizer for NoText {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        Ok(None)
    }
}

/// Renders with the CPU renderer and keeps the pixels of its latest frame.
struct Keeping {
    renderer: CpuRenderer<NoText>,
    pixels: Rc<RefCell<Vec<u8>>>,
    damage: Rc<RefCell<Vec<DamageRegion>>>,
}

impl UiRenderer for Keeping {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        let id = self.renderer.render(request)?;
        self.damage.borrow_mut().push(request.damage.clone());
        *self.pixels.borrow_mut() = self.renderer.frame(id)?.pixels.clone();
        Ok(id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.renderer.frame(id)
    }
}

/// A runtime drawing with real pixels at `scale`, showing `scene` first; its latest pixels and every damage.
#[allow(clippy::type_complexity)]
fn drawing(
    scene: Scene,
    scale: f64,
) -> (
    Rt,
    WindowId,
    Rc<RefCell<Vec<u8>>>,
    Rc<RefCell<Vec<DamageRegion>>>,
) {
    let pixels: Rc<RefCell<Vec<u8>>> = Rc::default();
    let damage: Rc<RefCell<Vec<DamageRegion>>> = Rc::default();
    let mut options = options(&Rc::default());
    let (kept, seen) = (Rc::clone(&pixels), Rc::clone(&damage));
    options.renderers = Box::new(move || {
        Box::new(Keeping {
            renderer: CpuRenderer::new(
                CpuConfig {
                    max_pixels: 1 << 20,
                    glyph_cache_bytes: 1 << 16,
                    image_cache_bytes: 1 << 16,
                    font_smoothing: true,
                },
                NoText,
            ),
            pixels: Rc::clone(&kept),
            damage: Rc::clone(&seen),
        })
    });
    let mut rt = Rt::new(FakeSystem::default(), options);
    rt.messages.push_back(scene);
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    rt.window_system_mut()
        .send(window, WindowEvent::Scale(scale));
    configure(&mut rt, window, 1, WINDOW);
    (rt, window, pixels, damage)
}

#[test]
fn a_window_drawn_from_damage_alone_is_the_window_drawn_whole() {
    let block = |key, x, y, w, h, color| (key, Rect::new(x, y, w, h), color);
    let mut scenes = Vec::new();
    let mut scene = Scene::two();
    // moves, at fractional points too
    for step in 0..6 {
        let f = f64::from(step);
        scene.blocks[0].1 = Rect::new(10.0 + 17.3 * f, 10.0 + 5.7 * f, 20.0, 20.0);
        scenes.push(scene.clone());
    }
    // a block coming in over another, one going away, children that draw nothing
    scene.blocks.push(block(3, 140.0, 50.0, 40.0, 40.0, GREY));
    scene.spacers = 2;
    scenes.push(scene.clone());
    scene.blocks.remove(1);
    scenes.push(scene.clone());
    // the board fades, and its subtree moves while it fades
    for (step, opacity) in [0.8, 0.5, 0.3].into_iter().enumerate() {
        scene.opacity = opacity;
        scene.blocks[0].1.origin.x = 120.0 - 25.5 * step as f64;
        scenes.push(scene.clone());
    }
    // it fills itself under its blocks, then the group goes
    scene.fill = Some(Color::new(0.2, 0.6, 0.3, 0.6));
    scenes.push(scene.clone());
    scene.blocks[1].1.size.width = 12.5;
    scenes.push(scene.clone());
    scene.opacity = 1.0;
    scene.spacers = 0;
    scenes.push(scene.clone());
    scene.fill = None;
    scenes.push(scene.clone());

    for scale in [1.0, 1.5, 2.0] {
        let (mut rt, window, pixels, damage) = drawing(Scene::two(), scale);
        for (time, scene) in scenes.iter().enumerate() {
            // now and then the window system is not ready, and the frame waits to be shown
            if time % 5 == 3 {
                rt.window_system_mut().refuse_next = true;
            }
            show(&mut rt, window, scene.clone(), time as u64 + 1);
            rt.window_system_mut().release(window);
            rt.step(steps()).unwrap();
            assert!(
                damage
                    .borrow()
                    .iter()
                    .skip(1)
                    .all(|d| matches!(d, DamageRegion::Rects(_))),
                "scale {scale}, step {time}: drawn from damage"
            );
            // the same scene drawn whole by a window that shows it from its first frame
            let (_, _, whole, whole_damage) = drawing(scene.clone(), scale);
            assert_eq!(whole_damage.borrow().as_slice(), [DamageRegion::Full]);
            let (drawn, whole) = (pixels.borrow(), whole.borrow());
            assert_eq!(drawn.len(), whole.len());
            let width = (WINDOW.width * scale).ceil() as usize;
            if let Some(i) = drawn.iter().zip(whole.iter()).position(|(a, b)| a != b) {
                panic!(
                    "scale {scale}, step {time}: pixel ({}, {}) differs from the window drawn whole",
                    i / 4 % width,
                    i / 4 / width
                );
            }
        }
    }
}
