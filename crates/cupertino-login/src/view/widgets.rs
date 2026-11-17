//! The widgets the screens are made of besides the standard controls: the backdrop with its kept wallpaper picture,
//! the fading parts of the interface, absolute placement, text at a size of its own, the avatar, the field's frame on its
//! material and its adornment, the buttons beside it and the power item.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use cupertino_geometry::rounded::{PathSeg, RoundedRect};
use cupertino_kit::core::{
    next_resource_id, AccessibleStates, Action, Change, Element, Event, EventCx, Glass, Live,
    MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{Keysym, Mods, MouseButton, PointerEvent};
use cupertino_kit::render::{
    Effect, FillRule, ImageResource, Paint, PaintOp, PathVerb, PixelFormat, ResourceId, Sampling,
    Shape,
};
use cupertino_kit::text::{
    AttributedText, FontWeight, GlyphFormat, GlyphKey, GlyphRaster, GlyphRasterizer,
    ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, TextSystem, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};

use crate::layout::{BUTTON_GLYPH, PILL_RADIUS};
use crate::spinner;
use crate::wallpaper::{self, Source};

const RETURN: Keysym = Keysym(0xff0d);
const KP_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
pub(crate) const ESCAPE: Keysym = Keysym(0xff1b);
/// Return or Enter with Option held, as the screens' keys see it first: no key has this symbol. A screen that does
/// not take it gets the plain key.
pub(crate) const OPTION_RETURN: Keysym = Keysym(0x0100_ff0d);

/// Fill behind the initials (approximate).
const AVATAR_FILL: [f32; 3] = [0.60, 0.62, 0.66];
/// The mark on the avatar of a user who has a session: a disc of this fill with a white check, at its lower right.
const LOGGED_IN_FILL: [f32; 3] = [0.20, 0.72, 0.35];
const LOGGED_IN_RADIUS: f64 = 9.0;
const LOGGED_IN_CHECK_WIDTH: f64 = 1.8;

/// The tint over the control material of the field and its buttons: the secondary label colour (white at this alpha in
/// the dark appearance), the tertiary one when disabled, laid over at this strength, with a darken layer of this
/// alpha inside the tint, which takes its white down to a light grey (approximate); with Increase Contrast black at
/// this alpha; with Reduce Transparency an opaque light field of this grey (approximate).
const TINT_SECONDARY_ALPHA: f32 = 0.55;
const TINT_TERTIARY_ALPHA: f32 = 0.25;
const TINT_STRENGTH: f32 = 0.2;
const TINT_DARKEN: f32 = 0.4;
const TINT_CONTRAST_ALPHA: f32 = 0.4;
const OPAQUE_FIELD: f32 = 0.92;
/// The text of an opaque field: black at these alphas, its text and its placeholder.
pub(crate) const OPAQUE_TEXT_ALPHA: f32 = 0.75;
pub(crate) const OPAQUE_PLACEHOLDER_ALPHA: f32 = 0.45;

/// The display options the screens follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Access {
    pub increase_contrast: bool,
    pub reduce_transparency: bool,
}

/// How the tint over a control's material looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum FieldTint {
    /// The secondary label colour; the tertiary one when disabled.
    Label { disabled: bool },
    /// Increase Contrast: black.
    Contrast,
    /// Reduce Transparency: an opaque light field, its text dark.
    Opaque,
}

impl FieldTint {
    pub(crate) fn of(access: Access, disabled: bool) -> FieldTint {
        if access.reduce_transparency {
            FieldTint::Opaque
        } else if access.increase_contrast {
            FieldTint::Contrast
        } else {
            FieldTint::Label { disabled }
        }
    }

    /// The colour laid over the material.
    pub(crate) fn color(self) -> Color {
        match self {
            FieldTint::Label { disabled } => {
                let alpha = if disabled {
                    TINT_TERTIARY_ALPHA
                } else {
                    TINT_SECONDARY_ALPHA
                };
                let grey = 1.0 - TINT_DARKEN;
                Color::new(grey, grey, grey, alpha * TINT_STRENGTH)
            }
            FieldTint::Contrast => Color::new(0.0, 0.0, 0.0, TINT_CONTRAST_ALPHA),
            FieldTint::Opaque => Color::new(OPAQUE_FIELD, OPAQUE_FIELD, OPAQUE_FIELD, 1.0),
        }
    }

    /// The field's colour with the tint laid over the material's colour `under` (0 to 1).
    #[cfg(test)]
    pub(crate) fn over(self, under: [f32; 3]) -> [f32; 3] {
        let tint = self.color();
        let [r, g, b, a] = [tint.r, tint.g, tint.b, tint.a];
        let rgb = [r, g, b];
        let mut out = [0.0; 3];
        for k in 0..3 {
            out[k] = under[k] + (rgb[k] - under[k]) * a;
        }
        out
    }
}

/// Caps Lock badge: a rounded square of corner radius 4 in 16-unit space, centred in the field's trailing slot.
const CAPS_BADGE_UNITS: f64 = 16.0;
const CAPS_BADGE_RADIUS: f64 = 4.0;
/// The up arrow over a bar that is cut out of the badge, in badge units.
const CAPS_GLYPH: [&[(f64, f64)]; 2] = [
    &[
        (8.0, 3.0),
        (12.5, 8.5),
        (10.0, 8.5),
        (10.0, 11.0),
        (6.0, 11.0),
        (6.0, 8.5),
        (3.5, 8.5),
    ],
    &[(5.5, 12.5), (10.5, 12.5), (10.5, 14.0), (5.5, 14.0)],
];
/// Edge of the spinner square in the field's trailing slot.
const SPINNER_SIZE: f64 = 16.0;
/// Edge of the padlock of a locked account.
const PADLOCK_SIZE: f64 = 14.0;

/// The power glyph: the ring's radius, stroke and the angle of its gap at the top, and how many segments draw the
/// ring.
const POWER_RADIUS: f64 = 5.5;
const POWER_STROKE: f64 = 1.6;
const POWER_GAP_DEGREES: f64 = 60.0;
const POWER_RING_STEPS: usize = 30;
/// The power item's fill while it has the keyboard focus or its menu is open (white at this alpha).
const POWER_HIGHLIGHT_ALPHA: f32 = 0.2;

/// The selected picker row: its frame with continuous corners of this radius, outlined 1 pt in white at this alpha.
const SELECTION_RADIUS: f64 = 12.0;
const SELECTION_BORDER_WIDTH: f64 = 1.0;
const SELECTION_BORDER_ALPHA: f32 = 0.1;

/// The user name's shadow: black at this opacity, not offset, of this blur radius in points. A blur radius is a
/// Gaussian of half its deviation (`sigma_for_radius`), so this is a deviation of 3 pt: a soft glow that hugs the
/// letters, gone about 9 pt from them.
const NAME_SHADOW_ALPHA: f32 = 0.5;
pub(crate) const NAME_SHADOW_BLUR: f64 = 6.0;

/// The glyphs of the submit and help buttons, in a square of [`BUTTON_GLYPH`] points: the ring's stroke, white at this
/// alpha (the help button's alpha is approximate, the submit button's).
const GLYPH_STROKE: f64 = 1.5;
const GLYPH_ALPHA: f32 = 0.5;
/// How many segments draw a glyph's curves.
const GLYPH_STEPS: usize = 32;

pub(crate) const WHITE: Color = Color::new(1.0, 1.0, 1.0, 1.0);

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn rgba(c: [f32; 3], alpha: f32) -> Color {
    Color::new(c[0], c[1], c[2], alpha)
}

fn fill(cx: &mut PaintCx<'_>, shape: Shape, color: Color) {
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(color),
    });
}

fn circle(center: Point, radius: f64) -> Shape {
    Shape::RoundedRect {
        rect: Rect::new(
            center.x - radius,
            center.y - radius,
            2.0 * radius,
            2.0 * radius,
        ),
        radius,
    }
}

/// What a full screen stands on: `source` over a screen of `size` points, with its dimming at opacity `dim`.
#[derive(Clone, Debug)]
pub(crate) struct Ground {
    pub source: Source,
    pub size: Size,
    pub dim: f64,
}

impl Ground {
    fn same(&self, other: &Ground) -> bool {
        self.source.key() == other.source.key() && self.size == other.size && self.dim == other.dim
    }
}

/// The longest side of a picture the screens make, device pixels.
const MAX_PICTURE_SIDE: f64 = 16384.0;
/// How many pictures and pieces of material the screens keep: a few outputs' pictures and fields.
const KEPT_PICTURES: usize = 8;

/// Side of the squares the picture is laid in, points: a frame that changes a part of the screen (a typed character, a
/// fading clock) lays again only the squares under it, copied from the kept picture.
const PICTURE_TILE: f64 = 256.0;

/// What a kept picture is of.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    /// The screen's picture before the dimming comes in.
    Undimmed,
    /// With the dimming fully in.
    Dimmed,
    /// With the dimming partly in, at this opacity's bits: one frame of the dimming coming in.
    Frame(u32),
    /// The control material over these device pixels of the screen.
    Material([u32; 4]),
    /// Glass letters, by a hash of their text, size, weight and place.
    Glass(u64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct KeptKey {
    source: u64,
    width: u32,
    height: u32,
    scale: u64,
    kind: Kind,
}

struct Kept {
    key: KeptKey,
    id: ResourceId,
    image: ImageResource,
}

thread_local! {
    /// The pictures made last, least recently used first.
    static KEPT: RefCell<Vec<Kept>> = const { RefCell::new(Vec::new()) };
    /// How many pictures of a whole screen were made from a source on this thread.
    static MADE: Cell<u64> = const { Cell::new(0) };
    /// How many pictures of glass letters were made on this thread.
    static GLASS_MADE: Cell<u64> = const { Cell::new(0) };
}

/// How many pictures of glass letters (the large time) were made on this thread: once per text, size and place on a
/// screen, so once a minute for the time.
#[cfg(test)]
pub(crate) fn glass_made() -> u64 {
    GLASS_MADE.get()
}

/// How many pictures of a whole screen were made from their source on this thread: once per screen size, scale and
/// wallpaper, and once more when the dimming has come in.
pub(crate) fn pictures_made() -> u64 {
    MADE.get()
}

/// Drop every kept picture (the lock screen ends, the greeter starts a session).
pub(crate) fn forget_pictures() {
    KEPT.with_borrow_mut(Vec::clear);
}

/// The device pixels a screen of `size` points covers at `scale`: whole pixels, at least one each way.
fn device_size(size: Size, scale: f64) -> (u32, u32) {
    let side = |v: f64| (v * scale).ceil().clamp(1.0, MAX_PICTURE_SIDE) as u32;
    (side(size.width), side(size.height))
}

/// Opaque premultiplied pixels of `width` x `height`, made in place by `fill`.
fn made(width: u32, height: u32, fill: impl FnOnce(&mut [u8])) -> ImageResource {
    let len = width as usize * height as usize * 4;
    // memory the system hands over zeroed, untouched until `fill` writes it: a whole screen of it is not set to zero
    // first, and its pages are first touched by the workers that fill it
    // SAFETY: zeroed bytes are valid `u8`s
    let mut pixels: Arc<[u8]> = unsafe { Arc::<[u8]>::new_zeroed_slice(len).assume_init() };
    if let Some(out) = Arc::get_mut(&mut pixels) {
        fill(out);
    }
    ImageResource {
        revision: 1,
        width,
        height,
        stride: width * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels,
    }
}

/// The kept picture of `key`, made by `make` when there is none.
fn kept(key: KeptKey, make: impl FnOnce() -> ImageResource) -> (ResourceId, ImageResource) {
    let found = KEPT.with_borrow_mut(|kept| {
        let i = kept.iter().position(|k| k.key == key)?;
        let entry = kept.remove(i);
        let found = (entry.id, entry.image.clone());
        kept.push(entry);
        Some(found)
    });
    if let Some(found) = found {
        return found;
    }
    let image = make();
    if matches!(key.kind, Kind::Undimmed | Kind::Dimmed) {
        MADE.set(MADE.get() + 1);
    }
    if matches!(key.kind, Kind::Glass(_)) {
        GLASS_MADE.set(GLASS_MADE.get() + 1);
    }
    let id = next_resource_id();
    KEPT.with_borrow_mut(|kept| {
        let screen = |k: &KeptKey| {
            (k.source, k.width, k.height, k.scale) == (key.source, key.width, key.height, key.scale)
        };
        // a frame of the dimming replaces the one before; the screen with its dimming in needs neither its frames nor
        // its undimmed picture
        match key.kind {
            Kind::Frame(_) => {
                kept.retain(|k| !(screen(&k.key) && matches!(k.key.kind, Kind::Frame(_))))
            }
            Kind::Dimmed => kept.retain(|k| {
                !(screen(&k.key) && matches!(k.key.kind, Kind::Frame(_) | Kind::Undimmed))
            }),
            // the time of the minute before is not shown again
            Kind::Glass(_) => {
                kept.retain(|k| !(screen(&k.key) && matches!(k.key.kind, Kind::Glass(_))))
            }
            Kind::Undimmed | Kind::Material(_) => {}
        }
        kept.push(Kept {
            key,
            id,
            image: image.clone(),
        });
        if kept.len() > KEPT_PICTURES {
            kept.remove(0);
        }
    });
    (id, image)
}

/// The picture of `ground` at `scale`, of whole device pixels: kept before the dimming comes in and once it is in; each
/// frame in between is made from the kept undimmed picture.
fn picture(ground: &Ground, scale: f64) -> (ResourceId, ImageResource) {
    let (width, height) = device_size(ground.size, scale);
    let key = KeptKey {
        source: ground.source.key(),
        width,
        height,
        scale: scale.to_bits(),
        kind: Kind::Undimmed,
    };
    let dim = ground.dim.clamp(0.0, 1.0) as f32;
    let source = &ground.source;
    if dim >= 1.0 {
        return kept(
            KeptKey {
                kind: Kind::Dimmed,
                ..key
            },
            || {
                made(width, height, |out| {
                    wallpaper::render_into(source, width, height, scale, 1.0, out)
                })
            },
        );
    }
    let (id, undimmed) = kept(key, || {
        made(width, height, |out| {
            wallpaper::render_into(source, width, height, scale, 0.0, out)
        })
    });
    if dim <= 0.0 {
        return (id, undimmed);
    }
    // every square of the frame lays the same picture
    kept(
        KeptKey {
            kind: Kind::Frame(dim.to_bits()),
            ..key
        },
        || {
            made(width, height, |out| {
                wallpaper::dim_into(&undimmed.pixels, width, height, scale, dim, out)
            })
        },
    )
}

/// The control material of `ground` over `strip` (screen points) at `scale`, kept, and the device pixels it covers.
fn material(
    ground: &Ground,
    scale: f64,
    strip: Rect,
) -> Option<(ResourceId, ImageResource, [u32; 4])> {
    let (width, height) = device_size(ground.size, scale);
    let to = |v: f64, limit: u32, up: bool| {
        let v = v * scale;
        (if up { v.ceil() } else { v.floor() }).clamp(0.0, f64::from(limit)) as u32
    };
    let region = [
        to(strip.min_x(), width, false),
        to(strip.min_y(), height, false),
        to(strip.max_x(), width, true),
        to(strip.max_y(), height, true),
    ];
    if region[0] >= region[2] || region[1] >= region[3] {
        return None;
    }
    let key = KeptKey {
        source: ground.source.key(),
        width,
        height,
        scale: scale.to_bits(),
        kind: Kind::Material(region),
    };
    let (id, image) = kept(key, || {
        let pixels = wallpaper::frosted(&ground.source, width, height, scale, region);
        ImageResource {
            revision: 1,
            width: region[2] - region[0],
            height: region[3] - region[1],
            stride: (region[2] - region[0]) * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels),
        }
    });
    Some((id, image, region))
}

/// The squares of [`PICTURE_TILE`] points that cover a screen of `size`, row by row; those at the right and bottom edges
/// are cut to the screen.
fn tiles(size: Size) -> impl Iterator<Item = Rect> {
    let count = |side: f64| (side.max(0.0) / PICTURE_TILE).ceil() as usize;
    let (columns, rows) = (count(size.width), count(size.height));
    (0..rows).flat_map(move |row| {
        (0..columns).map(move |column| {
            let (x, y) = (column as f64 * PICTURE_TILE, row as f64 * PICTURE_TILE);
            Rect::new(
                x,
                y,
                PICTURE_TILE.min(size.width - x),
                PICTURE_TILE.min(size.height - y),
            )
        })
    })
}

/// One square of the screen's picture, at `rect`: the kept picture's pixels under it, one a device pixel, copied and
/// never resampled. Its edges are on device pixels, so neighbouring squares meet without a seam at any scale.
struct PictureTile {
    ground: Ground,
    rect: Rect,
}

impl<M: 'static> Widget<M> for PictureTile {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if !self.ground.same(&old.ground) {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let scale = cx.env().scale;
        let (id, image) = picture(&self.ground, scale);
        let edge = |v: f64, limit: u32| (v * scale).round().clamp(0.0, f64::from(limit));
        let (x0, y0) = (
            edge(self.rect.min_x(), image.width),
            edge(self.rect.min_y(), image.height),
        );
        let (x1, y1) = (
            edge(self.rect.max_x(), image.width),
            edge(self.rect.max_y(), image.height),
        );
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        cx.image_part(
            id,
            &image,
            Rect::new(x0, y0, x1 - x0, y1 - y0),
            Rect::new(
                x0 / scale - self.rect.min_x(),
                y0 / scale - self.rect.min_y(),
                (x1 - x0) / scale,
                (y1 - y0) / scale,
            ),
            Sampling::Nearest,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// `rect` with continuous corners of `radius`, as a path to fill or clip by.
pub(crate) fn continuous(rect: Rect, radius: f64) -> Shape {
    let verbs = RoundedRect::continuous(rect, radius)
        .path()
        .segments
        .into_iter()
        .map(|segment| match segment {
            PathSeg::MoveTo(p) => PathVerb::MoveTo(p),
            PathSeg::LineTo(p) => PathVerb::LineTo(p),
            PathSeg::CubicTo(a, b, c) => PathVerb::CubicTo(a, b, c),
            PathSeg::Close => PathVerb::Close,
        })
        .collect();
    Shape::Path {
        verbs,
        fill_rule: FillRule::NonZero,
    }
}

/// The content of a full-screen window: the wallpaper's kept picture under everything, and the first look at every key
/// and press. Any of them is activity; while the interface is hidden the key or press only shows it again and goes no
/// further. Escape and the keys `keys` maps go to the application instead of the focused control.
pub(crate) struct Backdrop<M> {
    /// What the lock screen and the greeter stand on; a panel paints nothing.
    pub ground: Option<Ground>,
    pub hidden: bool,
    pub on_activity: Option<M>,
    /// What a key the application takes means; `None` passes the key on to the focus.
    pub keys: Box<dyn Fn(Keysym, Option<char>) -> Option<M>>,
    /// What assistive technology calls the screen, and says of it.
    pub label: String,
    pub description: Option<String>,
    pub role: Role,
    pub modal: bool,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Backdrop<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        let mut children: Vec<Element<M>> = self
            .ground
            .as_ref()
            .map(|ground| {
                tiles(ground.size)
                    .map(|rect| {
                        Element::new(PictureTile {
                            ground: ground.clone(),
                            rect,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        children.append(&mut self.children);
        children
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        // the squares of the picture follow the ground themselves
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() != Phase::Capture {
            return Propagation::Continue;
        }
        let activity = |cx: &mut EventCx<'_, M>| {
            if let Some(message) = &self.on_activity {
                cx.emit(message.clone());
            }
        };
        match event {
            Event::Key(key, _) if key.pressed => {
                activity(cx);
                if self.hidden {
                    return Propagation::Stop;
                }
                let option_return = key.mods.contains(Mods::ALT)
                    && matches!(key.keysym, Keysym(0xff0d) | Keysym(0xff8d));
                let taken = option_return
                    .then(|| (self.keys)(OPTION_RETURN, None))
                    .flatten()
                    .or_else(|| (self.keys)(key.keysym, key.text));
                match taken {
                    Some(message) => {
                        cx.emit(message);
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                }
            }
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                activity(cx);
                if self.hidden {
                    Propagation::Stop
                } else {
                    Propagation::Continue
                }
            }
            Event::Pointer(PointerEvent::Motion { .. }, _) => {
                activity(cx);
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        // the lock screen and the greeter stand on a darkened picture whatever the session's appearance
        if self.ground.is_some() {
            cx.pin_appearance(true);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        if self.modal {
            cx.states(AccessibleStates {
                modal: true,
                ..AccessibleStates::default()
            });
        }
    }
}

/// Width of the outline an alert on glass draws round itself with Increase Contrast, in points: a whole device pixel or
/// more on every display.
pub(crate) const ALERT_CONTRAST_RIM: f64 = 1.0;

/// An alert's body on glass, `width` wide and `height` tall or, without one, as tall as its content, which it pads by
/// `padding`: its corners rounded by `radius`, and the popover surface's fill where the window cannot show glass.
pub(crate) struct AlertBody<M> {
    pub width: f64,
    pub height: Option<f64>,
    pub radius: f64,
    pub padding: cupertino_kit::geometry::Insets,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for AlertBody<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            height: self.height.map_or(SizeSpec::Fit, SizeSpec::Fixed),
            insets: self.padding,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.width != old.width || self.height != old.height || self.padding != old.padding {
            Change::Layout
        } else if self.radius != old.radius {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        if cx.glass(Glass::regular().radius(self.radius)) {
            // with Increase Contrast the glass takes the contrast face, which over a window of the same appearance is
            // that window's tone: the alert marks its edge with the outline its buttons have
            let theme = cx.env().theme.clone();
            let rim = theme
                .colors
                .get(ColorRole::ControlBorder, StateVariant::NORMAL);
            if let (true, Some(rim)) = (theme.preferences.appearance.is_high_contrast(), rim) {
                let half = ALERT_CONTRAST_RIM / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(
                            half,
                            half,
                            size.width - ALERT_CONTRAST_RIM,
                            size.height - ALERT_CONTRAST_RIM,
                        ),
                        radius: (self.radius - half).max(0.0),
                    },
                    width: ALERT_CONTRAST_RIM,
                    paint: Paint::Solid(rim),
                });
            }
            return;
        }
        if let Some(color) = cx.env().theme.surface(Surface::Popover).fill {
            fill(
                cx,
                Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: self.radius,
                },
                color,
            );
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A part of a screen at `rect` (its children placed in it), drawn as one layer the size of `rect` while it is partly
/// faded; fully shown it is drawn straight over the wallpaper, which looks the same, so a change in it repaints only what
/// it covers. A fade composites the layer over the kept picture, which it does not make again.
pub(crate) struct Fade<M> {
    pub rect: Rect,
    pub opacity: f64,
    /// Its first control takes the focus when the window opens.
    pub initial: bool,
    /// A focus scope: Tab goes round its controls, and a new one, by its key, takes the focus as it opens.
    pub scope: bool,
    pub children: Vec<Element<M>>,
}

impl<M> Fade<M> {
    pub(crate) fn new(rect: Rect, opacity: f64, children: Vec<Element<M>>) -> Self {
        Fade {
            rect,
            opacity,
            initial: false,
            scope: false,
            children,
        }
    }
}

impl<M: 'static> Widget<M> for Fade<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.opacity != old.opacity {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let opacity = self.opacity.clamp(0.0, 1.0);
        if opacity < 1.0 {
            cx.set_group(opacity as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.initial {
            cx.initial_focus();
        }
        if self.scope {
            cx.focus_scope();
        }
    }
}

/// Its children at `rect` in an absolute parent, stacked along `axis` and placed across it by `align`. `initial` gives
/// its first control the focus when the window opens.
pub(crate) struct Place<M> {
    pub rect: Rect,
    pub axis: Axis,
    pub align: Align,
    pub justify: Justify,
    pub initial: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Place<M> {
    /// Centred both ways.
    pub(crate) fn new(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            rect,
            axis: Axis::Vertical,
            align: Align::Center,
            justify: Justify::Center,
            initial: false,
            children,
        }
    }

    /// From the top, against the leading edge, as a paragraph.
    pub(crate) fn text(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            align: Align::Start,
            justify: Justify::Start,
            ..Place::new(rect, children)
        }
    }

    /// Filling the box: a button at its frame.
    pub(crate) fn filled(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            align: Align::Stretch,
            ..Place::new(rect, children)
        }
    }
}

impl<M: 'static> Widget<M> for Place<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: self.axis,
                spacing: 0.0,
                align: self.align,
                justify: self.justify,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect
            || self.axis != old.axis
            || self.align != old.align
            || self.justify != old.justify
        {
            Change::Layout
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.initial {
            cx.initial_focus();
        }
    }
}

/// Text at a size and weight of its own, in `color` or the theme's text colour, centred in its box. Its first
/// baseline sits `ascent` below the box's top, so a caller places it by its baseline.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Label {
    pub text: String,
    pub size: f64,
    pub weight: FontWeight,
    pub color: Option<Color>,
    /// The box, in the parent's points.
    pub rect: Rect,
    /// From the box's top to the first baseline.
    pub ascent: f64,
    /// Wraps at the box's width; otherwise one line that ends in an ellipsis.
    pub wrap: bool,
    /// Read out as soon as it shows or changes.
    pub live: Option<Live>,
    /// Drawn over a soft black shadow (the user name).
    pub shadow: bool,
    /// How its lines stand in its box.
    pub align: TextAlign,
    /// Drawn lower by [`Shift::by`] when the text of [`Shift`] wraps (the sub hint under a main hint of two lines).
    pub shift: Option<Shift>,
}

/// Another label's text, at its width and size: when it wraps, the label that carries this is drawn lower.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shift {
    pub text: String,
    pub width: f64,
    pub size: f64,
    pub by: f64,
}

impl Label {
    fn laid_out(&self, cx: &PaintCx<'_>) -> Option<ParagraphLayout> {
        let content = AttributedText::plain(
            self.text.clone(),
            TextStyle {
                weight: self.weight,
                ..TextStyle::new(self.size as f32)
            },
        );
        let style = ParagraphStyle {
            align: self.align,
            wrap: if self.wrap { Wrap::Word } else { Wrap::None },
            ellipsis: !self.wrap,
            ..ParagraphStyle::default()
        };
        let paragraph = cx.env().text.paragraph(&content, &style).ok()?;
        Some(paragraph.layout(Some(self.rect.width() as f32)))
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: Some(self.ascent),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(layout) = self.laid_out(cx) else {
            return;
        };
        let color = self.color.or_else(|| {
            cx.env()
                .theme
                .colors
                .get(ColorRole::Text, StateVariant::NORMAL)
        });
        let Some(color) = color else {
            return;
        };
        if self.shadow {
            cx.set_group(
                1.0,
                vec![Effect::DropShadow {
                    offset: Point::new(0.0, 0.0),
                    blur: NAME_SHADOW_BLUR,
                    color: Color::new(0.0, 0.0, 0.0, NAME_SHADOW_ALPHA),
                }],
            );
        }
        let first = layout.lines().first().map_or(0.0, |line| line.baseline);
        let dropped = self.shift.as_ref().map_or(0.0, |shift| {
            let above = Label {
                text: shift.text.clone(),
                size: shift.size,
                rect: Rect::new(0.0, 0.0, shift.width, 0.0),
                wrap: true,
                shift: None,
                ..self.clone()
            };
            let wraps = above
                .laid_out(cx)
                .is_some_and(|layout| layout.lines().len() > 1);
            if wraps {
                shift.by
            } else {
                0.0
            }
        });
        let origin = Point::new(0.0, self.ascent - first + dropped);
        // a wrapped label draws the lines its box holds, so it never runs into what stands under it
        let shown = layout
            .lines()
            .iter()
            .take_while(|line| !self.wrap || origin.y + line.baseline <= self.rect.height());
        for line in shown {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: origin + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
        if let Some(live) = self.live {
            cx.live(live);
        }
    }
}

/// Text in glass letters (the large time): the screen's own picture under the letters, blurred and lifted toward
/// white, with a light rim along their top and left edges and a dark one along their bottom and right
/// ([`wallpaper::glass_letters`]). It is made from the kept picture, never from what lies under the window, and kept, so
/// a frame lays it again without making it; without a picture the letters are white. Centred in its box, its first
/// baseline `ascent` below the box's top.
#[derive(Clone, Debug)]
pub(crate) struct GlassText {
    pub text: String,
    pub size: f64,
    pub weight: FontWeight,
    /// The box, in the parent's points.
    pub rect: Rect,
    pub ascent: f64,
    pub ground: Ground,
    /// Where the parent's origin lies on the screen, points.
    pub origin: Point,
}

impl GlassText {
    fn same(&self, other: &GlassText) -> bool {
        (
            self.text.as_str(),
            self.size,
            self.weight,
            self.ascent,
            self.origin,
        ) == (
            other.text.as_str(),
            other.size,
            other.weight,
            other.ascent,
            other.origin,
        ) && self.ground.source.key() == other.ground.source.key()
            && self.ground.size == other.ground.size
    }

    /// The glass letters at `scale` and the device pixels of the screen they cover, kept; `None` without glyphs.
    fn letters(
        &self,
        cx: &PaintCx<'_>,
        layout: &ParagraphLayout,
        scale: f64,
    ) -> Option<(ResourceId, ImageResource, [u32; 4])> {
        use std::hash::{Hash, Hasher};
        let (width, height) = device_size(self.ground.size, scale);
        let at = Point::new(self.origin.x + self.rect.x(), self.origin.y + self.rect.y());
        let first = layout.lines().first().map_or(0.0, |line| line.baseline);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (
            self.text.as_str(),
            self.size.to_bits(),
            self.weight.0,
            self.ascent.to_bits(),
            at.x.to_bits(),
            at.y.to_bits(),
            self.rect.width().to_bits(),
            self.rect.height().to_bits(),
        )
            .hash(&mut hasher);
        let key = KeptKey {
            source: self.ground.source.key(),
            width,
            height,
            scale: scale.to_bits(),
            kind: Kind::Glass(hasher.finish()),
        };
        // the box in device pixels: the letters are cut to it and to the screen
        let bound = [
            ((at.x * scale).floor().max(0.0) as u32).min(width),
            ((at.y * scale).floor().max(0.0) as u32).min(height),
            ((self.rect.max_x() + self.origin.x) * scale)
                .ceil()
                .clamp(0.0, f64::from(width)) as u32,
            ((self.rect.max_y() + self.origin.y) * scale)
                .ceil()
                .clamp(0.0, f64::from(height)) as u32,
        ];
        let found = KEPT.with_borrow_mut(|kept| {
            let i = kept.iter().position(|k| k.key == key)?;
            let entry = kept.remove(i);
            let found = (entry.id, entry.image.clone());
            kept.push(entry);
            Some(found)
        });
        let region = |image: &ImageResource, left: u32, top: u32| {
            [left, top, left + image.width, top + image.height]
        };
        if let Some((id, image)) = found {
            let left = REGIONS.with_borrow(|r| r.get(&id).copied())?;
            return Some((id, image.clone(), region(&image, left[0], left[1])));
        }
        // each glyph's coverage at its device pixel, as the renderer places it
        let mut raster = GlyphRaster::new(cx.env().text.clone());
        let mut glyphs = Vec::new();
        for line in layout.lines() {
            for run in &line.runs {
                let size_px = f64::from(run.run.size) * scale;
                for g in &run.run.glyphs {
                    let px = (at.x + run.origin.x + f64::from(g.x)) * scale;
                    let py = (at.y + self.ascent - first + run.origin.y + f64::from(g.y)) * scale;
                    let ix = px.floor();
                    let key = GlyphKey::for_draw(
                        run.run.font,
                        g.id,
                        size_px,
                        px - ix,
                        true,
                        [1.0, 1.0, 1.0],
                    );
                    let Ok(Some(glyph)) = raster.rasterize(&key) else {
                        continue;
                    };
                    if glyph.format != GlyphFormat::Coverage {
                        continue;
                    }
                    let x = ix as i64 + i64::from(glyph.bearing_x);
                    let y = py.round() as i64 - i64::from(glyph.bearing_y);
                    glyphs.push((x, y, glyph));
                }
            }
        }
        let clamp = |v: i64, lo: u32, hi: u32| v.clamp(i64::from(lo), i64::from(hi)) as u32;
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, g) in &glyphs {
            x0 = x0.min(clamp(*x, bound[0], bound[2]));
            y0 = y0.min(clamp(*y, bound[1], bound[3]));
            x1 = x1.max(clamp(x + i64::from(g.width), bound[0], bound[2]));
            y1 = y1.max(clamp(y + i64::from(g.height), bound[1], bound[3]));
        }
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let (rw, rh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut coverage = vec![0u8; rw * rh];
        for (x, y, g) in &glyphs {
            for row in 0..g.height as i64 {
                let sy = y + row - i64::from(y0);
                if sy < 0 || sy >= rh as i64 {
                    continue;
                }
                for col in 0..g.width as i64 {
                    let sx = x + col - i64::from(x0);
                    if sx < 0 || sx >= rw as i64 {
                        continue;
                    }
                    let v = g.pixels[(row * i64::from(g.stride) + col) as usize];
                    let at = sy as usize * rw + sx as usize;
                    coverage[at] = coverage[at].saturating_add(v);
                }
            }
        }
        let region_px = [x0, y0, x1, y1];
        let source = &self.ground.source;
        let (id, image) = kept(key, || {
            let pixels =
                wallpaper::glass_letters(source, width, height, scale, region_px, &coverage);
            ImageResource {
                revision: 1,
                width: x1 - x0,
                height: y1 - y0,
                stride: (x1 - x0) * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(pixels),
            }
        });
        REGIONS.with_borrow_mut(|r| {
            r.retain(|k, _| KEPT.with_borrow(|kept| kept.iter().any(|e| e.id == *k)));
            r.insert(id, [x0, y0]);
        });
        Some((id, image, region_px))
    }
}

thread_local! {
    /// Where each kept picture of glass letters lies on its screen: its top-left device pixel.
    static REGIONS: RefCell<std::collections::HashMap<ResourceId, [u32; 2]>> =
        RefCell::new(std::collections::HashMap::new());
}

impl<M: 'static> Widget<M> for GlassText {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if !self.same(old) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: Some(self.ascent),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let label = Label {
            color: Some(WHITE),
            ..label_of(&self.text, self.size, self.weight, self.rect, self.ascent)
        };
        let Some(layout) = label.laid_out(cx) else {
            return;
        };
        let scale = cx.env().scale;
        let Some((id, image, region)) = self.letters(cx, &layout, scale) else {
            return;
        };
        let at = Point::new(self.origin.x + self.rect.x(), self.origin.y + self.rect.y());
        let dst = Rect::new(
            f64::from(region[0]) / scale - at.x,
            f64::from(region[1]) / scale - at.y,
            f64::from(image.width) / scale,
            f64::from(image.height) / scale,
        );
        cx.image(id, &image, dst, Sampling::Nearest);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// A plain label of `text` at `size` and `weight` in `rect`, its first baseline `ascent` below its top.
fn label_of(text: &str, size: f64, weight: FontWeight, rect: Rect, ascent: f64) -> Label {
    Label {
        text: text.to_owned(),
        size,
        weight,
        color: None,
        rect,
        ascent,
        wrap: false,
        live: None,
        shadow: false,
        align: TextAlign::Center,
        shift: None,
    }
}

/// An avatar at `rect`: a plain disc, its picture or initials as children, with no ring and no shadow, and the mark
/// of a user with a session.
pub(crate) struct Avatar<M> {
    pub rect: Rect,
    /// No picture: the disc is filled behind the initials.
    pub filled: bool,
    pub logged_in: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Avatar<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            // the initials' line box sits at its own place in the disc
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.filled != old.filled || self.logged_in != old.logged_in {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let radius = self.rect.width() / 2.0;
        let center = Point::new(radius, radius);
        if self.filled {
            fill(cx, circle(center, radius), rgba(AVATAR_FILL, 1.0));
        }
        if self.logged_in {
            let mark = Point::new(
                2.0 * radius - LOGGED_IN_RADIUS,
                2.0 * radius - LOGGED_IN_RADIUS,
            );
            fill(
                cx,
                circle(mark, LOGGED_IN_RADIUS),
                rgba(LOGGED_IN_FILL, 1.0),
            );
            let at = |dx: f64, dy: f64| {
                Point::new(
                    mark.x + LOGGED_IN_RADIUS * dx,
                    mark.y + LOGGED_IN_RADIUS * dy,
                )
            };
            cx.push(PaintOp::Stroke {
                shape: Shape::Path {
                    verbs: vec![
                        PathVerb::MoveTo(at(-0.45, 0.02)),
                        PathVerb::LineTo(at(-0.1, 0.38)),
                        PathVerb::LineTo(at(0.5, -0.35)),
                    ],
                    fill_rule: FillRule::NonZero,
                },
                width: LOGGED_IN_CHECK_WIDTH,
                paint: Paint::Solid(WHITE),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Fill of the field's capsule where there is no picture to make its material from: white at this opacity.
const FIELD_FILL_ALPHA: f32 = 0.18;

/// The control material under a field or a button: made from `ground`'s picture over `strip` (screen points, wide enough
/// for the field's shake) and laid where it lies on the screen, whatever the frame's own place, `origin` (screen points).
/// It is never what lies under the window: under a lock surface that is the unlocked session.
#[derive(Clone, Debug)]
pub(crate) struct FieldMaterial {
    pub ground: Ground,
    pub strip: Rect,
    pub origin: Point,
}

/// The capsule of a field or a button at `rect` with continuous corners, on the control material: its field (if any)
/// inside it by `insets`, and for a field (`initial`) the stop its scope gives the focus to as it opens.
pub(crate) struct FieldFrame<M> {
    pub rect: Rect,
    pub insets: Insets,
    pub material: Option<FieldMaterial>,
    pub tint: FieldTint,
    pub initial: bool,
    /// The submit button's glyph at this box (the frame's points): drawn under the field, whose submit strip takes
    /// the press there.
    pub submit: Option<Rect>,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for FieldFrame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            insets: self.insets,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        let material = |m: &Option<FieldMaterial>| {
            m.as_ref()
                .map(|m| (m.ground.source.key(), m.ground.size, m.strip, m.origin))
        };
        if self.rect != old.rect || self.insets != old.insets {
            Change::Layout
        } else if self.tint != old.tint
            || self.submit != old.submit
            || material(&self.material) != material(&old.material)
        {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let radius = PILL_RADIUS.min(size.height / 2.0);
        let capsule = continuous(Rect::new(0.0, 0.0, size.width, size.height), radius);
        let scale = cx.env().scale;
        let laid = self
            .material
            .as_ref()
            .and_then(|m| material(&m.ground, scale, m.strip).map(|found| (m, found)));
        match laid {
            Some((m, (id, image, region))) => {
                cx.push(PaintOp::PushClip(capsule.clone()));
                let dst = Rect::new(
                    f64::from(region[0]) / scale - m.origin.x,
                    f64::from(region[1]) / scale - m.origin.y,
                    f64::from(image.width) / scale,
                    f64::from(image.height) / scale,
                );
                cx.image(id, &image, dst, Sampling::Nearest);
                cx.push(PaintOp::PopClip);
            }
            None => fill(
                cx,
                capsule.clone(),
                Color::new(1.0, 1.0, 1.0, FIELD_FILL_ALPHA),
            ),
        }
        fill(cx, capsule, self.tint.color());
        // an opaque light field takes dark text
        if self.tint == FieldTint::Opaque {
            cx.pin_appearance(false);
        }
        if let Some(button) = self.submit {
            paint_glyph(cx, GlyphKind::Submit, button, self.tint);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.initial {
            cx.initial_focus();
        }
    }
}

/// The selection's border along the inside of a tile of `size`.
fn selection_border(cx: &mut PaintCx<'_>, size: Size) {
    let half = SELECTION_BORDER_WIDTH / 2.0;
    cx.push(PaintOp::Stroke {
        shape: continuous(
            Rect::new(
                half,
                half,
                size.width - SELECTION_BORDER_WIDTH,
                size.height - SELECTION_BORDER_WIDTH,
            ),
            SELECTION_RADIUS - half,
        ),
        width: SELECTION_BORDER_WIDTH,
        paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, SELECTION_BORDER_ALPHA)),
    });
}

/// A glyph of the buttons beside the field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GlyphKind {
    /// An arrow pointing right in a ring: submit.
    Submit,
    /// An arrow pointing left in a ring: back to the picker.
    Back,
    /// A question mark in a ring: the password help.
    Help,
}

/// A curve sampled into line segments: `f(t)` for `t` from 0 to 1.
fn sampled(verbs: &mut Vec<PathVerb>, f: impl Fn(f64) -> Point) {
    for step in 0..=GLYPH_STEPS {
        let point = f(step as f64 / GLYPH_STEPS as f64);
        verbs.push(if step == 0 {
            PathVerb::MoveTo(point)
        } else {
            PathVerb::LineTo(point)
        });
    }
}

/// `kind`'s glyph centred in `area`, [`BUTTON_GLYPH`] points across: a ring and its mark, stroked in white at
/// [`GLYPH_ALPHA`].
fn paint_glyph(cx: &mut PaintCx<'_>, kind: GlyphKind, area: Rect, tint: FieldTint) {
    let unit = BUTTON_GLYPH / 22.0;
    let (cx0, cy0) = (
        area.min_x() + area.width() / 2.0,
        area.min_y() + area.height() / 2.0,
    );
    // the glyph's own 22-unit square, centred
    let at = |x: f64, y: f64| Point::new(cx0 + (x - 11.0) * unit, cy0 + (y - 11.0) * unit);
    let color = if tint == FieldTint::Opaque {
        Color::new(0.0, 0.0, 0.0, OPAQUE_TEXT_ALPHA)
    } else {
        Color::new(1.0, 1.0, 1.0, GLYPH_ALPHA)
    };
    let ring_radius = 11.0 - GLYPH_STROKE / 2.0 / unit;
    let mut verbs = Vec::new();
    sampled(&mut verbs, |t| {
        let a = t * std::f64::consts::TAU;
        at(11.0 + ring_radius * a.cos(), 11.0 + ring_radius * a.sin())
    });
    verbs.push(PathVerb::Close);
    match kind {
        GlyphKind::Submit => {
            verbs.extend([
                PathVerb::MoveTo(at(6.5, 11.0)),
                PathVerb::LineTo(at(15.5, 11.0)),
                PathVerb::MoveTo(at(11.8, 7.2)),
                PathVerb::LineTo(at(15.6, 11.0)),
                PathVerb::LineTo(at(11.8, 14.8)),
            ]);
        }
        GlyphKind::Back => {
            verbs.extend([
                PathVerb::MoveTo(at(15.5, 11.0)),
                PathVerb::LineTo(at(6.5, 11.0)),
                PathVerb::MoveTo(at(10.2, 7.2)),
                PathVerb::LineTo(at(6.4, 11.0)),
                PathVerb::LineTo(at(10.2, 14.8)),
            ]);
        }
        GlyphKind::Help => {
            // the hook: most of a circle above the stem, from its left round over the top to its right and down
            let (hx, hy, hr) = (11.0, 8.4, 3.0);
            sampled(&mut verbs, |t| {
                let a = (200.0 + t * 230.0f64).to_radians();
                at(hx + hr * a.cos(), hy + hr * a.sin())
            });
            verbs.extend([
                PathVerb::LineTo(at(11.0, 11.6)),
                PathVerb::LineTo(at(11.0, 13.0)),
            ]);
            fill(cx, circle(at(11.0, 15.8), 1.05 * unit), color);
        }
    }
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs,
            fill_rule: FillRule::NonZero,
        },
        width: GLYPH_STROKE,
        paint: Paint::Solid(color),
    });
}

/// A button's glyph at `rect`, drawn over its material; it takes nothing, its button does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Glyph {
    pub rect: Rect,
    pub kind: GlyphKind,
    pub tint: FieldTint,
}

impl<M: 'static> Widget<M> for Glyph {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self != old {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        paint_glyph(
            cx,
            self.kind,
            Rect::new(0.0, 0.0, size.width, size.height),
            self.tint,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// What the field's trailing slot shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Adornment {
    /// The spinner at this frame: an attempt is being checked.
    Spinner(u32),
    CapsLock,
    /// The account is locked: a padlock.
    Locked,
}

/// The field's trailing slot at `rect`, drawn over the field in the theme's secondary text colour.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Slot {
    pub rect: Rect,
    pub adornment: Adornment,
    /// What assistive technology hears of it.
    pub label: &'static str,
}

impl Slot {
    fn caps(&self, cx: &mut PaintCx<'_>, color: Color) {
        let size = cx.size();
        let side = CAPS_BADGE_UNITS;
        let (left, top) = ((size.width - side) / 2.0, (size.height - side) / 2.0);
        let r = CAPS_BADGE_RADIUS;
        let k = 0.552_284_75 * r;
        let (x0, y0, x1, y1) = (left, top, left + side, top + side);
        let mut verbs = vec![
            PathVerb::MoveTo(Point::new(x0 + r, y0)),
            PathVerb::LineTo(Point::new(x1 - r, y0)),
            PathVerb::CubicTo(
                Point::new(x1 - r + k, y0),
                Point::new(x1, y0 + r - k),
                Point::new(x1, y0 + r),
            ),
            PathVerb::LineTo(Point::new(x1, y1 - r)),
            PathVerb::CubicTo(
                Point::new(x1, y1 - r + k),
                Point::new(x1 - r + k, y1),
                Point::new(x1 - r, y1),
            ),
            PathVerb::LineTo(Point::new(x0 + r, y1)),
            PathVerb::CubicTo(
                Point::new(x0 + r - k, y1),
                Point::new(x0, y1 - r + k),
                Point::new(x0, y1 - r),
            ),
            PathVerb::LineTo(Point::new(x0, y0 + r)),
            PathVerb::CubicTo(
                Point::new(x0, y0 + r - k),
                Point::new(x0 + r - k, y0),
                Point::new(x0 + r, y0),
            ),
            PathVerb::Close,
        ];
        for polygon in CAPS_GLYPH {
            for (i, &(x, y)) in polygon.iter().enumerate() {
                let point = Point::new(left + x, top + y);
                verbs.push(if i == 0 {
                    PathVerb::MoveTo(point)
                } else {
                    PathVerb::LineTo(point)
                });
            }
            verbs.push(PathVerb::Close);
        }
        // even-odd: the glyph polygons lie inside the body, so they are holes
        fill(
            cx,
            Shape::Path {
                verbs,
                fill_rule: FillRule::EvenOdd,
            },
            color,
        );
    }

    fn spinner(&self, cx: &mut PaintCx<'_>, color: Color, frame: u32) {
        let size = cx.size();
        let center = Point::new(size.width / 2.0, size.height / 2.0);
        let unit = SPINNER_SIZE / spinner::CANVAS;
        let half_width = spinner::SPOKE_WIDTH / 2.0;
        for spoke in 0..spinner::SPOKES {
            // clockwise from 12 o'clock
            let angle = spoke as f64 * std::f64::consts::TAU / spinner::SPOKES as f64;
            let (sin, cos) = angle.sin_cos();
            let at = |radius: f64| {
                Point::new(
                    center.x + sin * radius * unit,
                    center.y - cos * radius * unit,
                )
            };
            cx.push(PaintOp::Stroke {
                shape: Shape::Path {
                    verbs: vec![
                        PathVerb::MoveTo(at(spinner::INNER_RADIUS + half_width)),
                        PathVerb::LineTo(at(spinner::OUTER_RADIUS - half_width)),
                    ],
                    fill_rule: FillRule::NonZero,
                },
                width: spinner::SPOKE_WIDTH * unit,
                paint: Paint::Solid(Color::new(
                    color.r,
                    color.g,
                    color.b,
                    color.a * spinner::opacity(frame, spoke),
                )),
            });
        }
    }
}

impl<M: 'static> Widget<M> for Slot {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self != old {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some(color) = cx
            .env()
            .theme
            .colors
            .get(ColorRole::SecondaryText, StateVariant::NORMAL)
        else {
            return;
        };
        match self.adornment {
            Adornment::CapsLock => self.caps(cx, color),
            Adornment::Spinner(frame) => self.spinner(cx, color, frame),
            Adornment::Locked => {
                let size = cx.size();
                padlock(cx, color, size)
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        match self.adornment {
            Adornment::CapsLock | Adornment::Locked => cx.role(Role::StaticText),
            Adornment::Spinner(_) => {
                cx.role(Role::ProgressIndicator);
                cx.states(AccessibleStates {
                    busy: true,
                    ..AccessibleStates::default()
                });
            }
        }
        cx.name(self.label);
    }
}

/// The padlock of a locked account, [`PADLOCK_SIZE`] square and centred in `size`: a body with rounded corners and a
/// shackle over it, in `color`.
fn padlock(cx: &mut PaintCx<'_>, color: Color, size: Size) {
    let s = PADLOCK_SIZE / 14.0;
    let (x0, y0) = (
        (size.width - PADLOCK_SIZE) / 2.0,
        (size.height - PADLOCK_SIZE) / 2.0,
    );
    let at = |x: f64, y: f64| Point::new(x0 + x * s, y0 + y * s);
    fill(
        cx,
        Shape::RoundedRect {
            rect: Rect::new(x0 + 2.0 * s, y0 + 6.0 * s, 10.0 * s, 8.0 * s),
            radius: 1.5 * s,
        },
        color,
    );
    let mut verbs = vec![
        PathVerb::MoveTo(at(4.2, 6.5)),
        PathVerb::LineTo(at(4.2, 4.6)),
    ];
    sampled(&mut verbs, |t| {
        let a = std::f64::consts::PI * (1.0 + t);
        at(7.0 + 2.8 * a.cos(), 4.6 + 2.8 * a.sin())
    });
    verbs.push(PathVerb::LineTo(at(9.8, 6.5)));
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs,
            fill_rule: FillRule::NonZero,
        },
        width: 1.5 * s,
        paint: Paint::Solid(color),
    });
}

/// A control drawn by the screen itself at `rect`: the power item (a glyph) or a picker tile (a selection square and
/// its children). A click, Return, Space or assistive technology's click send `on_press`; gaining the focus sends
/// `on_focus`; the pointer over it sends `on_hover`.
pub(crate) struct Pressable<M> {
    pub rect: Rect,
    pub look: Look,
    pub label: String,
    pub description: Option<String>,
    pub on_press: M,
    pub on_focus: Option<M>,
    pub on_hover: Option<M>,
    /// A Tab stop; one that is not leaves the focus where it is when pressed (the help button, so typing goes on in
    /// the field).
    pub focusable: bool,
    pub children: Vec<Element<M>>,
}

/// How a [`Pressable`] draws itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Look {
    /// The power glyph, highlighted while the menu is open.
    Power { open: bool },
    /// A picker tile: its frame with continuous corners, filled and outlined while selected.
    Tile { selected: bool },
    /// The help button: its children are its material and its glyph; `open` while the help shows.
    Help { open: bool },
}

#[derive(Default)]
pub(crate) struct PressState {
    focused: bool,
}

impl<M: Clone + 'static> Widget<M> for Pressable<M> {
    type State = PressState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> PressState {
        PressState::default()
    }

    fn update(&self, old: &Self, _: &mut PressState) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.look != old.look || self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &PressState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut PressState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Focus { gained, .. } => {
                state.focused = *gained;
                cx.request_paint();
                if let (true, Some(message)) = (*gained, &self.on_focus) {
                    cx.emit(message.clone());
                }
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Enter { .. } | PointerEvent::Motion { .. }, _) => {
                if let Some(message) = &self.on_hover {
                    cx.emit(message.clone());
                }
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Key(key, _)
                if key.pressed
                    && cx.node() == cx.target()
                    && matches!(key.keysym, RETURN | KP_ENTER | SPACE) =>
            {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, state: &PressState, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        match self.look {
            Look::Power { open } => {
                if open || (state.focused && cx.focus_visible()) {
                    fill(
                        cx,
                        Shape::RoundedRect {
                            rect: Rect::new(0.0, 0.0, size.width, size.height),
                            radius: size.height / 2.0,
                        },
                        Color::new(1.0, 1.0, 1.0, POWER_HIGHLIGHT_ALPHA),
                    );
                }
                power_glyph(cx, Point::new(size.width / 2.0, size.height / 2.0));
            }
            // the selection is the row's frame outlined, with no fill and no shadow
            Look::Tile { selected } => {
                if selected || (state.focused && cx.focus_visible()) {
                    selection_border(cx, size);
                }
            }
            // its children draw it: its material and its glyph
            Look::Help { .. } => {}
        }
    }

    fn semantics(&self, _: &PressState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        if let Some(description) = &self.description {
            cx.description(description.clone());
        }
        if let Look::Tile { selected, .. } = self.look {
            cx.states(AccessibleStates {
                selected,
                ..AccessibleStates::default()
            });
        }
        if let Look::Power { open } | Look::Help { open } = self.look {
            cx.states(AccessibleStates {
                expanded: Some(open),
                ..AccessibleStates::default()
            });
        }
        if self.focusable {
            cx.focusable();
        }
        cx.action(Action::Click);
    }
}

/// The power glyph centred on `center`: a ring open at the top with a bar through the gap, white, round caps.
fn power_glyph(cx: &mut PaintCx<'_>, center: Point) {
    let mut verbs = Vec::new();
    for step in 0..=POWER_RING_STEPS {
        let angle = (POWER_GAP_DEGREES / 2.0
            + step as f64 * (360.0 - POWER_GAP_DEGREES) / POWER_RING_STEPS as f64)
            .to_radians();
        let point = Point::new(
            center.x + POWER_RADIUS * angle.sin(),
            center.y - POWER_RADIUS * angle.cos(),
        );
        verbs.push(if step == 0 {
            PathVerb::MoveTo(point)
        } else {
            PathVerb::LineTo(point)
        });
    }
    verbs.push(PathVerb::MoveTo(Point::new(
        center.x,
        center.y - POWER_RADIUS - 1.0,
    )));
    verbs.push(PathVerb::LineTo(Point::new(center.x, center.y - 1.0)));
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs,
            fill_rule: FillRule::NonZero,
        },
        width: POWER_STROKE,
        paint: Paint::Solid(WHITE),
    });
}

/// A card's fill where its material is not made (a screen too large for a picture): white at this alpha over the
/// treated wallpaper (approximate); and the alpha of its secondary text.
const NOTICE_FILL_ALPHA: f32 = 0.18;
const NOTICE_SECONDARY_ALPHA: f32 = 0.75;

/// A notification on the lock screen, in the column of cards: its application, its title and its body, left-aligned,
/// each cut with an ellipsis where it does not fit. It is as tall as its text, and the gap below it is its own. It is
/// one node for assistive technology, named by `speech` and said politely as it arrives; it takes no input, so the
/// password field keeps the focus. It stands on the control material the field does, with the field's tint, made
/// from the screen's own picture.
pub(crate) struct NoticeCard {
    pub app: String,
    pub title: String,
    pub body: String,
    pub speech: String,
    pub ascent: f64,
    pub descent: f64,
    /// The material of the column of cards, its origin the column's.
    pub material: Option<FieldMaterial>,
    /// The bodies of the cards above it in the column, which place it.
    pub above: Vec<String>,
}

/// A line of a card's text: its size, weight and how many lines it may take.
struct Line {
    size: f64,
    weight: FontWeight,
    lines: u32,
}

impl NoticeCard {
    fn laid_out(&self, text: &TextSystem, words: &str, line: &Line) -> Option<ParagraphLayout> {
        use crate::lock_notices::{CARD_PADDING, CARD_WIDTH};
        let content = AttributedText::plain(
            words.to_owned(),
            TextStyle {
                weight: line.weight,
                ..TextStyle::new(line.size as f32)
            },
        );
        let style = ParagraphStyle {
            align: TextAlign::Start,
            wrap: if line.lines > 1 {
                Wrap::Word
            } else {
                Wrap::None
            },
            max_lines: Some(line.lines),
            ellipsis: true,
            ..ParagraphStyle::default()
        };
        let paragraph = text.paragraph(&content, &style).ok()?;
        Some(paragraph.layout(Some((CARD_WIDTH - 2.0 * CARD_PADDING) as f32)))
    }

    const BODY: Line = Line {
        size: crate::lock_notices::BODY_SIZE,
        weight: FontWeight::REGULAR,
        lines: crate::lock_notices::BODY_LINES,
    };

    /// How far down the column the card stands: the heights of the cards above it, each with its gap.
    fn offset_in_column(&self, text: &TextSystem) -> f64 {
        use crate::lock_notices::{card_height, CARD_GAP};
        self.above
            .iter()
            .map(|body| {
                let card = NoticeCard {
                    body: body.clone(),
                    above: Vec::new(),
                    material: None,
                    app: String::new(),
                    title: String::new(),
                    speech: String::new(),
                    ..*self
                };
                card_height(card.body_lines(text), self.ascent, self.descent) + CARD_GAP
            })
            .sum()
    }

    /// How many lines the body takes: none without one.
    fn body_lines(&self, text: &TextSystem) -> u32 {
        if self.body.trim().is_empty() {
            return 0;
        }
        self.laid_out(text, &self.body, &Self::BODY)
            .map_or(Self::BODY.lines, |l| l.lines().len() as u32)
            .clamp(1, Self::BODY.lines)
    }

    /// Paint `words` with its first line's top at `top`; returns the top of what follows.
    fn paint_text(
        &self,
        cx: &mut PaintCx<'_>,
        words: &str,
        line: &Line,
        top: f64,
        color: Color,
    ) -> f64 {
        use crate::lock_notices::CARD_PADDING;
        let Some(layout) = self.laid_out(&cx.env().text, words, line) else {
            return top;
        };
        let first = layout.lines().first().map_or(0.0, |l| l.baseline);
        let origin = Point::new(CARD_PADDING, top + self.ascent * line.size - first);
        let shown = layout.lines().len().min(line.lines as usize).max(1);
        for l in layout.lines().iter().take(shown) {
            for run in &l.runs {
                cx.push(PaintOp::Glyphs {
                    origin: origin + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
        top + shown as f64 * (self.ascent + self.descent) * line.size
    }
}

impl<M: 'static> Widget<M> for NoticeCard {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(crate::lock_notices::CARD_WIDTH),
            height: SizeSpec::Fit,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        fn text(c: &NoticeCard) -> (&String, &String, &String, &String, f64, f64) {
            (&c.app, &c.title, &c.body, &c.speech, c.ascent, c.descent)
        }
        let material = |m: &Option<FieldMaterial>| m.as_ref().map(|m| (m.strip, m.origin));
        let ground_same = match (&self.material, &old.material) {
            (Some(a), Some(b)) => a.ground.same(&b.ground),
            (None, None) => true,
            _ => false,
        };
        if text(self) != text(old) {
            Change::Layout
        } else if self.above != old.above
            || material(&self.material) != material(&old.material)
            || !ground_same
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        use crate::lock_notices::{card_height, CARD_GAP, CARD_WIDTH};
        let lines = self.body_lines(&cx.env().text);
        Measured {
            size: Size::new(
                CARD_WIDTH,
                card_height(lines, self.ascent, self.descent) + CARD_GAP,
            ),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        use crate::lock_notices::{
            APP_SIZE, CARD_GAP, CARD_PADDING, CARD_RADIUS, LINE_GAP, TITLE_SIZE,
        };
        let size = cx.size();
        let card = Shape::RoundedRect {
            rect: Rect::new(0.0, 0.0, size.width, (size.height - CARD_GAP).max(0.0)),
            radius: CARD_RADIUS,
        };
        let scale = cx.env().scale;
        let tint = FieldTint::Label { disabled: false };
        let laid = self
            .material
            .as_ref()
            .and_then(|m| material(&m.ground, scale, m.strip).map(|found| (m, found)));
        match laid {
            Some((m, (id, image, region))) => {
                // the card's place in the column: under the cards above it
                let offset = self.offset_in_column(&cx.env().text);
                cx.push(PaintOp::PushClip(card.clone()));
                let dst = Rect::new(
                    f64::from(region[0]) / scale - m.origin.x,
                    f64::from(region[1]) / scale - m.origin.y - offset,
                    f64::from(image.width) / scale,
                    f64::from(image.height) / scale,
                );
                cx.image(id, &image, dst, Sampling::Nearest);
                cx.push(PaintOp::PopClip);
                fill(cx, card, tint.color());
            }
            None => fill(cx, card, Color::new(1.0, 1.0, 1.0, NOTICE_FILL_ALPHA)),
        }
        let secondary = Color::new(1.0, 1.0, 1.0, NOTICE_SECONDARY_ALPHA);
        let app = Line {
            size: APP_SIZE,
            weight: FontWeight::REGULAR,
            lines: 1,
        };
        let title = Line {
            size: TITLE_SIZE,
            weight: FontWeight::SEMIBOLD,
            lines: 1,
        };
        let top = self.paint_text(cx, &self.app, &app, CARD_PADDING, secondary);
        let top = self.paint_text(cx, &self.title, &title, top + LINE_GAP, WHITE);
        if !self.body.trim().is_empty() {
            self.paint_text(cx, &self.body, &Self::BODY, top + LINE_GAP, secondary);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.speech.clone());
        cx.live(Live::Polite);
    }
}
