//! A frame's damage covers every pixel that changed. Seeded runs of small random changes to a scene of shapes (fills,
//! strokes, rings drawn past their box, shadows and blurs in a layer, groups with an opacity or a shadow, clipping
//! containers, containers that draw nothing while their children move, items added, removed and reordered) are drawn
//! from damage, and each frame is compared with the window drawn whole before and after it: a pixel that differs
//! between the two whole frames lies inside the damage, and every pixel, inside it or out, is the whole frame's exactly.

use crate::support;

use cupertino_kit::core::{
    Application, Change, Command, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx,
    Phase, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_kit::platform::{
    DecorationMode, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent, WindowSpec,
    WindowStates,
};
use cupertino_kit::render::{Effect, Paint, PaintOp, PixelRect, Shape as Outline};
use cupertino_kit::theme::Color;
use std::cell::RefCell;
use support::Run;

/// The window's size in points.
const WINDOW: Size = Size::new(220.0, 150.0);

/// Changes in each seeded run.
const STEPS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Fill,
    Rounded,
    Stroke,
    /// A ring drawn outside the item's box, as a focus ring is.
    Ring,
    /// A rounded fill with a drop shadow, in a layer of its own operations.
    Shadow,
    Blur,
}

const KINDS: [Kind; 6] = [
    Kind::Fill,
    Kind::Rounded,
    Kind::Stroke,
    Kind::Ring,
    Kind::Shadow,
    Kind::Blur,
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Holder {
    /// Draws nothing itself.
    Plain,
    /// Cuts its children to its box.
    Clip,
    /// Its children in a group with this opacity.
    Group(f32),
    /// Its children in a group with a drop shadow.
    Shadowed,
}

#[derive(Clone, Debug, PartialEq)]
struct Item {
    id: u32,
    rect: Rect,
    kind: Kind,
    color: [f32; 4],
    /// Its outline has rounded corners, else square ones.
    round: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Container {
    id: u32,
    rect: Rect,
    holder: Holder,
    items: Vec<Item>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Scene {
    items: Vec<Item>,
    containers: Vec<Container>,
}

/// A small deterministic generator, so a failing seed is run again as it was.
struct Rng {
    state: u64,
    /// Whether the items it makes may have rounded corners.
    curves: bool,
}

impl Rng {
    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// A value in `lo..hi`, in steps of a fifth of a point so edges fall inside pixels.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let steps = ((hi - lo) * 5.0) as usize;
        lo + self.below(steps.max(1)) as f64 / 5.0
    }

    fn color(&mut self) -> [f32; 4] {
        let mut c = || self.below(256) as f32 / 255.0;
        [c(), c(), c(), 0.4 + 0.6 * c()]
    }
}

fn item(rng: &mut Rng, id: u32, room: Size) -> Item {
    let (w, h) = (rng.range(6.0, 50.0), rng.range(6.0, 40.0));
    Item {
        id,
        rect: Rect::new(
            rng.range(-4.0, room.width - 4.0),
            rng.range(-4.0, room.height - 4.0),
            w,
            h,
        ),
        kind: KINDS[rng.below(KINDS.len())],
        color: rng.color(),
        round: rng.curves && rng.below(3) != 0,
    }
}

fn scene(rng: &mut Rng, next: &mut u32) -> Scene {
    let mut fresh = || {
        *next += 1;
        *next
    };
    let items = (0..6).map(|_| item(rng, fresh(), WINDOW)).collect();
    let holders = [
        Holder::Plain,
        Holder::Clip,
        Holder::Group(0.6),
        Holder::Shadowed,
    ];
    let containers = holders
        .into_iter()
        .map(|holder| {
            let rect = Rect::new(
                rng.range(0.0, 140.0),
                rng.range(0.0, 90.0),
                rng.range(40.0, 80.0),
                rng.range(30.0, 60.0),
            );
            let items = (0..2).map(|_| item(rng, fresh(), rect.size)).collect();
            Container {
                id: fresh(),
                rect,
                holder,
                items,
            }
        })
        .collect();
    Scene { items, containers }
}

/// Some item of `scene`, at random.
fn pick<'a>(rng: &mut Rng, scene: &'a mut Scene) -> Option<&'a mut Item> {
    let count = scene.items.len()
        + scene
            .containers
            .iter()
            .map(|c| c.items.len())
            .sum::<usize>();
    if count == 0 {
        return None;
    }
    let at = rng.below(count);
    let mut all = scene
        .items
        .iter_mut()
        .chain(scene.containers.iter_mut().flat_map(|c| c.items.iter_mut()));
    all.nth(at)
}

/// One small change to `scene`.
fn change(rng: &mut Rng, scene: &mut Scene, next: &mut u32) {
    match rng.below(9) {
        0 | 1 => {
            let (dx, dy) = (rng.range(-12.0, 12.0), rng.range(-12.0, 12.0));
            if let Some(item) = pick(rng, scene) {
                item.rect = item.rect.translate(dx, dy);
            }
        }
        2 => {
            let size = Size::new(rng.range(4.0, 50.0), rng.range(4.0, 40.0));
            if let Some(item) = pick(rng, scene) {
                item.rect.size = size;
            }
        }
        3 => {
            let color = rng.color();
            if let Some(item) = pick(rng, scene) {
                item.color = color;
            }
        }
        4 => {
            let kind = KINDS[rng.below(KINDS.len())];
            if let Some(item) = pick(rng, scene) {
                item.kind = kind;
            }
        }
        5 => {
            *next += 1;
            let at = rng.below(scene.containers.len() + 1);
            match scene.containers.get_mut(at) {
                Some(c) => {
                    let room = c.rect.size;
                    c.items.push(item(rng, *next, room));
                }
                None => scene.items.push(item(rng, *next, WINDOW)),
            }
        }
        6 => {
            let at = rng.below(scene.containers.len() + 1);
            let items = match scene.containers.get_mut(at) {
                Some(c) => &mut c.items,
                None => &mut scene.items,
            };
            if !items.is_empty() {
                let gone = rng.below(items.len());
                items.remove(gone);
            }
        }
        7 => {
            // a container moves or changes size, and its children with it
            let c = rng.below(scene.containers.len());
            let c = &mut scene.containers[c];
            if rng.below(2) == 0 {
                c.rect = c
                    .rect
                    .translate(rng.range(-10.0, 10.0), rng.range(-10.0, 10.0));
            } else {
                c.rect.size = Size::new(rng.range(30.0, 90.0), rng.range(20.0, 70.0));
            }
        }
        _ => {
            // two items trade places in paint order
            if scene.items.len() >= 2 {
                let (a, b) = (rng.below(scene.items.len()), rng.below(scene.items.len()));
                scene.items.swap(a, b);
            }
        }
    }
}

fn rgba(c: [f32; 4]) -> Color {
    Color::new(c[0], c[1], c[2], c[3])
}

/// One item: what it draws follows its kind and colour, its box its rect.
struct Shape {
    item: Item,
}

impl Widget<Step> for Shape {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.item.rect.width()),
            height: SizeSpec::Fixed(self.item.rect.height()),
            position: self.item.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        // the box is the spec's, which the tree compares; what is drawn in it is paint
        if self.item == old.item {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Step>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let bounds = Rect::new(0.0, 0.0, size.width, size.height);
        let paint = Paint::Solid(rgba(self.item.color));
        let rounded = if self.item.round {
            Outline::RoundedRect {
                rect: bounds,
                radius: size.width.min(size.height) / 4.0,
            }
        } else {
            Outline::Rect(bounds)
        };
        match self.item.kind {
            Kind::Fill => cx.push(PaintOp::Fill {
                shape: Outline::Rect(bounds),
                paint,
            }),
            Kind::Rounded => cx.push(PaintOp::Fill {
                shape: rounded,
                paint,
            }),
            Kind::Stroke => cx.push(PaintOp::Stroke {
                shape: rounded,
                width: 1.5,
                paint,
            }),
            Kind::Ring => cx.push(PaintOp::Stroke {
                shape: if self.item.round {
                    Outline::RoundedRect {
                        rect: bounds.outset(4.0),
                        radius: 6.0,
                    }
                } else {
                    Outline::Rect(bounds.outset(4.0))
                },
                width: 3.0,
                paint,
            }),
            Kind::Shadow | Kind::Blur => {
                let effect = match self.item.kind {
                    Kind::Shadow => Effect::DropShadow {
                        offset: Point::new(0.0, 3.0),
                        blur: 6.0,
                        color: Color::new(0.0, 0.0, 0.0, 0.5),
                    },
                    _ => Effect::Blur { radius: 3.0 },
                };
                cx.push(PaintOp::PushLayer {
                    opacity: 1.0,
                    effects: vec![effect],
                });
                cx.push(PaintOp::Fill {
                    shape: rounded,
                    paint,
                });
                cx.push(PaintOp::PopLayer);
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Image);
        cx.name(format!("item {}", self.item.id));
    }
}

/// A container of items, which draws nothing of its own; it may cut them to its box or hold them in a group.
struct Holding {
    rect: Rect,
    holder: Holder,
    children: Vec<Element<Step>>,
}

impl Widget<Step> for Holding {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            overflow: match self.holder {
                Holder::Clip => Overflow::Clip,
                _ => Overflow::Visible,
            },
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Step>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.holder == old.holder {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Step>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        match self.holder {
            Holder::Group(opacity) => cx.set_group(opacity, Vec::new()),
            Holder::Shadowed => cx.set_group(
                1.0,
                vec![Effect::DropShadow {
                    offset: Point::new(2.0, 4.0),
                    blur: 5.0,
                    color: Color::new(0.0, 0.0, 0.0, 0.6),
                }],
            ),
            Holder::Plain | Holder::Clip => {}
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
    }
}

/// The whole window: a press anywhere asks for the next scene.
struct Stage {
    children: Vec<Element<Step>>,
}

impl Widget<Step> for Stage {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Step>> {
        std::mem::take(&mut self.children)
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Step>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _)
                if cx.phase() != Phase::Bubble =>
            {
                cx.emit(Step);
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name("Scene");
    }
}

thread_local! {
    /// The scenes of this thread's run, in order; each press shows the next.
    static SCENES: RefCell<Vec<Scene>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Debug, PartialEq)]
struct Step;

struct Scenes;

impl Application for Scenes {
    /// The scene shown, by its place in [`SCENES`].
    type Model = usize;
    type Message = Step;

    fn init() -> (usize, Vec<Command<Step>>) {
        (0, Vec::new())
    }

    fn update(shown: &mut usize, _: Step) -> Vec<Command<Step>> {
        *shown += 1;
        Vec::new()
    }

    fn view(shown: &usize) -> Vec<Window<Step>> {
        let scene = SCENES.with_borrow(|scenes| scenes[*shown].clone());
        let shape = |item: &Item| {
            Element::new(Shape { item: item.clone() }).key(format!("item/{}", item.id))
        };
        let mut children: Vec<Element<Step>> = scene
            .containers
            .iter()
            .map(|c| {
                Element::new(Holding {
                    rect: c.rect,
                    holder: c.holder,
                    children: c.items.iter().map(shape).collect(),
                })
                .key(format!("container/{}", c.id))
            })
            .collect();
        children.extend(scene.items.iter().map(shape));
        vec![Window {
            key: WindowKey("scenes".into()),
            spec: WindowSpec {
                title: "Scenes".into(),
                app_id: "org.cupertino.tests.Scenes".into(),
                size: WINDOW,
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
            content: Element::new(Stage { children }),
            menus: None,
        }]
    }
}

fn frames(run: &Run<Scenes>) -> usize {
    run.driver
        .runtime()
        .window_system()
        .frames(run.window)
        .unwrap()
        .len()
}

/// Draws the window whole, as a configure into another state and back does, and returns its pixels. (A configure that
/// changes nothing draws nothing.)
fn whole(run: &mut Run<Scenes>) -> Vec<u8> {
    let before = frames(run);
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    for states in [
        WindowStates {
            fullscreen: true,
            ..active
        },
        active,
    ] {
        run.driver
            .window_system_mut()
            .configure(run.window, None, states)
            .unwrap();
        run.settle();
    }
    let presented = run.driver.runtime().window_system().frames(run.window);
    let last = presented.unwrap()[before..]
        .last()
        .expect("the configure drew a frame");
    let all = u64::from(last.width) * u64::from(last.height);
    let drawn: u64 = last
        .damage
        .iter()
        .map(|r| u64::from(r.width) * u64::from(r.height))
        .sum();
    assert_eq!(drawn, all, "a configure draws the window whole");
    last.pixels.clone()
}

fn covered(damage: &[PixelRect], x: u32, y: u32) -> bool {
    damage
        .iter()
        .any(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
}

/// Runs `STEPS` changes from `seed` at `scale`, with rounded corners when `curves`, and checks that each frame drawn
/// from damage changed nothing outside it; returns how far the redrawn pixels came from the whole frames, at most.
fn check(seed: u64, scale: f64, curves: bool) -> u8 {
    // spread the seed's bits, as the generator never leaves a state of all zeros
    let mut rng = Rng {
        state: seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
        curves,
    };
    let mut furthest = 0;
    let mut next = 0;
    let mut scenes = vec![scene(&mut rng, &mut next)];
    for _ in 0..STEPS {
        let mut s = scenes.last().unwrap().clone();
        change(&mut rng, &mut s, &mut next);
        scenes.push(s);
    }
    SCENES.set(scenes.clone());
    let mut run = Run::<Scenes>::new();
    run.send(WindowEvent::Scale(scale));
    run.settle();
    let mut before = whole(&mut run);
    for step in 1..=STEPS {
        let from = frames(&run);
        let position = Point::new(1.0, 1.0);
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        run.send(pointer(PointerEvent::Motion { position }));
        run.send(pointer(PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed: true,
            serial: 1,
            mods: Mods::NONE,
        }));
        run.settle();
        assert_eq!(
            *run.model(),
            step,
            "seed {seed}: the press showed the next scene"
        );
        let presented = run.driver.runtime().window_system().frames(run.window);
        let presented = &presented.unwrap()[from..];
        let damage: Vec<PixelRect> = presented.iter().flat_map(|f| f.damage.clone()).collect();
        let kept = presented
            .last()
            .map_or_else(|| before.clone(), |f| f.pixels.clone());
        let after = whole(&mut run);
        let width = (WINDOW.width * scale).round() as u32;
        let mut stale = Vec::new();
        let mut moved = 0;
        let mut blended = 0;
        for (i, ((was, now), kept)) in before
            .chunks(4)
            .zip(after.chunks(4))
            .zip(kept.chunks(4))
            .enumerate()
        {
            let (x, y) = (i as u32 % width, i as u32 / width);
            let inside = covered(&damage, x, y);
            if was != now && !inside {
                stale.push((x, y, was.to_vec(), now.to_vec()));
            }
            if !inside && kept != now {
                moved += 1;
            }
            if inside {
                let off = kept
                    .iter()
                    .zip(now)
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                blended = blended.max(off);
            }
        }
        assert!(
            stale.is_empty(),
            "seed {seed}, scale {scale}, step {step}: {} pixels changed outside the damage {damage:?}, \
             first {:?}\nbefore {:?}\nafter {:?}",
            stale.len(),
            &stale[..stale.len().min(4)],
            scenes[step - 1],
            scenes[step],
        );
        assert_eq!(
            moved, 0,
            "seed {seed}, step {step}: a pixel outside the damage changed"
        );
        furthest = furthest.max(blended);
        before = after;
    }
    furthest
}

/// The seeds each test runs.
const SEEDS: std::ops::RangeInclusive<u64> = 1..=4;

fn every_pixel_at(scale: f64) {
    for curves in [false, true] {
        for seed in SEEDS {
            let furthest = check(seed, scale, curves);
            assert_eq!(
                furthest, 0,
                "seed {seed}, scale {scale}, curves {curves}: a redrawn pixel is {furthest} levels from the whole frame"
            );
        }
    }
}

#[test]
fn every_changed_pixel_lies_inside_the_damage_at_scale_1() {
    every_pixel_at(1.0);
}

#[test]
fn every_changed_pixel_lies_inside_the_damage_at_a_fractional_scale() {
    every_pixel_at(1.5);
}

#[test]
fn every_changed_pixel_lies_inside_the_damage_at_scale_2() {
    every_pixel_at(2.0);
}

/// A blur or shadow in a group, near a damaged rectangle's border, is computed from the pixels past the border too.
#[test]
fn square_shapes_drawn_again_in_part_are_the_whole_frame_s_at_whole_scales() {
    for scale in [1.0, 2.0] {
        for seed in 5..=8 {
            let furthest = check(seed, scale, false);
            assert_eq!(
                furthest, 0,
                "seed {seed}, scale {scale}: a redrawn pixel is {furthest} levels from the whole frame"
            );
        }
    }
}
