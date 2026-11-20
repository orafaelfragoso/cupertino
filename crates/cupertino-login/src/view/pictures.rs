//! The pictures the screens stand on and are made of: the wallpaper's picture of each screen (before its dimming comes
//! in, each frame of the dimming coming in, and with the dimming in), the control material under the fields, the round
//! buttons and the notification cards, and the glass letters of the large time. None is made while a frame is painted:
//! each is made on Kit's CPU pool, a whole screen's picture in bands of rows of [`PICTURE_TILE`] squares that the
//! pool's threads share, and kept by the program in a [`ScreenPictures`].
//!
//! After each update the program tells [`ScreenPictures::want`] what each of its screens shows ([`Wants`], which
//! [`Screen::wants`](super::Screen::wants) and [`PickerScreen::wants`](super::PickerScreen::wants) give) and runs the
//! commands it gets back; each picture comes back as a [`Made`] message, which the program hands to
//! [`ScreenPictures::made`]. The views take what a screen shows from [`ScreenPictures::shown`]: while a picture is
//! made, the screen shows the last one it had (the time of the minute before, the frame of the dimming before), so
//! nothing flashes; [`ScreenPictures::ready`] says when everything a screen wants is in.

use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cupertino_kit::core::{next_resource_id, Command, Environment, WindowKey};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::render::{self, zeroed_pixels, ImageResource, PaintOp, PixelFormat, ResourceId};
use cupertino_kit::task;
use cupertino_kit::text::FontWeight;

use super::widgets::{label_of, WHITE};
use crate::wallpaper::{self, Source};

/// Side of the squares the picture is laid in, points: a frame that changes a part of the screen (a typed character, a
/// fading clock) lays again only the squares under it. A row of squares is also a band of the picture, made on the CPU
/// pool apart from the others.
pub(crate) const PICTURE_TILE: f64 = 256.0;
/// The longest side of a picture the screens make, device pixels.
const MAX_PICTURE_SIDE: f64 = 16384.0;
/// How many pictures and pieces of material the screens keep besides those they show: a few outputs' pictures and
/// fields.
const KEPT_PICTURES: usize = 8;

/// What a picture is of.
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

/// A picture of one screen: its wallpaper, its device pixels, its scale and what it is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PictureKey {
    source: u64,
    width: u32,
    height: u32,
    scale: u64,
    kind: Kind,
}

impl PictureKey {
    /// Of the same wallpaper over the same device pixels at the same scale.
    fn same_screen(&self, other: &PictureKey) -> bool {
        (self.source, self.width, self.height, self.scale)
            == (other.source, other.width, other.height, other.scale)
    }

    fn with(self, kind: Kind) -> PictureKey {
        PictureKey { kind, ..self }
    }
}

/// A picture laid on a screen: the id the renderer knows it by, its pixels, the device pixels of the screen it covers
/// (\`[x0, y0, x1, y1]\`) and the scale it was made at.
#[derive(Clone)]
pub(crate) struct Laid {
    pub id: ResourceId,
    pub image: ImageResource,
    pub region: [u32; 4],
    pub scale: f64,
}

impl PartialEq for Laid {
    fn eq(&self, other: &Laid) -> bool {
        self.id == other.id
    }
}

impl fmt::Debug for Laid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Laid({:?} at {:?} x{})",
            self.id, self.region, self.scale
        )
    }
}

/// A screen's whole picture, opaque, in bands of rows of [`PICTURE_TILE`] squares: each square lies within one band.
pub(crate) struct Whole {
    key: PictureKey,
    bands: Vec<Laid>,
}

impl Whole {
    /// The device pixels a point of the picture covers.
    pub(crate) fn scale(&self) -> f64 {
        f64::from_bits(self.key.scale)
    }

    /// The picture's device pixels, across and down.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.key.width, self.key.height)
    }

    /// The band that holds the device rows \`y0..y1\`.
    pub(crate) fn band(&self, y0: u32, y1: u32) -> Option<&Laid> {
        self.bands
            .iter()
            .find(|band| band.region[1] <= y0 && y1 <= band.region[3])
    }
}

/// What the large time's glass letters are: the text at its size and weight, centred in its box (in the points of the
/// clock's part, which starts at \`origin\` on the screen), its first baseline \`ascent\` below the box's top.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Glass {
    pub text: String,
    pub size: f64,
    pub weight: FontWeight,
    pub rect: Rect,
    pub ascent: f64,
    pub origin: Point,
}

impl Glass {
    /// The box's top left corner on the screen, points.
    pub(crate) fn at(&self) -> Point {
        Point::new(self.origin.x + self.rect.x(), self.origin.y + self.rect.y())
    }

    /// What tells these letters from others.
    fn hash(&self) -> u64 {
        let at = self.at();
        let mut hasher = DefaultHasher::new();
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
        hasher.finish()
    }

    /// The box in device pixels of a screen of \`width\` x \`height\` at \`scale\`: the letters are cut to it.
    fn bound(&self, width: u32, height: u32, scale: f64) -> [u32; 4] {
        let at = self.at();
        [
            ((at.x * scale).floor().max(0.0) as u32).min(width),
            ((at.y * scale).floor().max(0.0) as u32).min(height),
            ((self.rect.max_x() + self.origin.x) * scale)
                .ceil()
                .clamp(0.0, f64::from(width)) as u32,
            ((self.rect.max_y() + self.origin.y) * scale)
                .ceil()
                .clamp(0.0, f64::from(height)) as u32,
        ]
    }
}

/// What one screen's pictures are made from: the screen's size in points, its device scale, its wallpaper and its
/// dimming, the strips of the screen whose control material its controls stand on, and its glass letters.
#[derive(Clone, Debug)]
pub struct Wants {
    pub(crate) size: Size,
    pub(crate) scale: f64,
    pub(crate) source: Source,
    pub(crate) dim: f64,
    pub(crate) strips: Vec<Rect>,
    /// The strip of the notification cards' material, which shows the one before while it is made.
    pub(crate) notices: Option<Rect>,
    pub(crate) glass: Option<Glass>,
}

/// The device pixels a screen of \`size\` points covers at \`scale\`: whole pixels, at least one each way.
fn device_size(size: Size, scale: f64) -> (u32, u32) {
    let side = |v: f64| (v * scale).ceil().clamp(1.0, MAX_PICTURE_SIDE) as u32;
    (side(size.width), side(size.height))
}

/// A usable scale: a scale no display has is taken as 1.
fn usable(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The pictures a screen wants: its whole picture, each strip's material (\`None\`: a strip of no pixels, which has
/// none), the notification cards' material, and its glass letters.
struct Targets {
    ground: PictureKey,
    strips: Vec<(Rect, Option<PictureKey>)>,
    notices: Option<Option<PictureKey>>,
    glass: Option<PictureKey>,
}

impl Wants {
    fn scale(&self) -> f64 {
        usable(self.scale)
    }

    /// The screen's own key, of its picture before the dimming comes in.
    fn base(&self) -> PictureKey {
        let scale = self.scale();
        let (width, height) = device_size(self.size, scale);
        PictureKey {
            source: self.source.key(),
            width,
            height,
            scale: scale.to_bits(),
            kind: Kind::Undimmed,
        }
    }

    /// The device pixels of the screen \`strip\` (points) covers; \`None\` for none.
    fn region(&self, strip: Rect) -> Option<[u32; 4]> {
        let scale = self.scale();
        let (width, height) = device_size(self.size, scale);
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
        (region[0] < region[2] && region[1] < region[3]).then_some(region)
    }

    fn targets(&self) -> Targets {
        let base = self.base();
        let dim = self.dim.clamp(0.0, 1.0) as f32;
        let ground = if dim >= 1.0 {
            base.with(Kind::Dimmed)
        } else if dim <= 0.0 {
            base
        } else {
            base.with(Kind::Frame(dim.to_bits()))
        };
        let material = |strip: Rect| {
            self.region(strip)
                .map(|region| base.with(Kind::Material(region)))
        };
        Targets {
            ground,
            strips: self
                .strips
                .iter()
                .map(|&strip| (strip, material(strip)))
                .collect(),
            notices: self.notices.map(material),
            glass: self
                .glass
                .as_ref()
                .map(|glass| base.with(Kind::Glass(glass.hash()))),
        }
    }
}

/// What one screen shows now: the pictures it was last given, which it keeps while newer ones are made.
#[derive(Clone)]
pub struct Shown {
    wants: Option<Wants>,
    ground: Option<Arc<Whole>>,
    /// Each strip's material, by the picture it is.
    materials: Vec<(Rect, Option<PictureKey>, Option<Laid>)>,
    notices: Option<(Option<PictureKey>, Option<Laid>)>,
    glass: Option<(PictureKey, Option<Laid>)>,
}

/// What a screen the program never named shows: nothing.
static NOTHING: Shown = Shown {
    wants: None,
    ground: None,
    materials: Vec::new(),
    notices: None,
    glass: None,
};

impl Shown {
    /// Whether every picture the screen wants is in: its picture, its materials and its glass letters, each as it last
    /// wanted it. A screen that wants nothing yet is not ready.
    pub fn ready(&self) -> bool {
        let Some(wants) = &self.wants else {
            return false;
        };
        let t = wants.targets();
        let ground = self.ground.as_ref().is_some_and(|g| g.key == t.ground);
        let strips = t.strips.iter().all(|(strip, key)| {
            key.is_none()
                || self
                    .materials
                    .iter()
                    .any(|(s, k, _)| s == strip && k == key)
        });
        let notices = t.notices.is_none_or(|key| {
            key.is_none() || self.notices.as_ref().is_some_and(|(k, _)| *k == key)
        });
        let glass = t
            .glass
            .is_none_or(|key| self.glass.as_ref().is_some_and(|(k, _)| *k == key));
        ground && strips && notices && glass
    }

    /// The screen's whole picture.
    pub(crate) fn ground(&self) -> Option<Arc<Whole>> {
        self.ground.clone()
    }

    /// The material of \`strip\`.
    pub(crate) fn material(&self, strip: Rect) -> Option<Laid> {
        self.materials
            .iter()
            .find(|(s, _, _)| *s == strip)
            .and_then(|(_, _, laid)| laid.clone())
    }

    /// The material of the notification cards.
    pub(crate) fn notices(&self) -> Option<Laid> {
        self.notices.as_ref().and_then(|(_, laid)| laid.clone())
    }

    /// The glass letters.
    pub(crate) fn letters(&self) -> Option<Laid> {
        self.glass.as_ref().and_then(|(_, laid)| laid.clone())
    }
}

impl fmt::Debug for Shown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shown")
            .field("ground", &self.ground.as_ref().map(|g| g.key))
            .field("materials", &self.materials.len())
            .field("glass", &self.glass.as_ref().map(|(k, _)| *k))
            .field("ready", &self.ready())
            .finish()
    }
}

/// A picture made on the CPU pool, as a message for the program to hand to [`ScreenPictures::made`].
#[derive(Clone)]
pub struct Made(Arrival);

#[derive(Clone)]
enum Arrival {
    /// Band \`index\` of a whole picture.
    Band {
        key: PictureKey,
        index: usize,
        laid: Laid,
    },
    /// A material or glass letters; \`None\` for a picture of nothing.
    Part { key: PictureKey, laid: Option<Laid> },
    /// The coverage of the glass letters' glyphs, white on clear, over the screen's device pixels from its top left
    /// corner to the bottom right one of \`bound\`, from which the letters are made.
    Coverage {
        key: PictureKey,
        source: Source,
        coverage: Option<ImageResource>,
        bound: [u32; 4],
    },
}

impl fmt::Debug for Made {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Arrival::Band { key, index, .. } => write!(f, "Made({key:?} band {index})"),
            Arrival::Part { key, .. } => write!(f, "Made({key:?})"),
            Arrival::Coverage { key, .. } => write!(f, "Made({key:?} coverage)"),
        }
    }
}

#[derive(Clone)]
enum Picture {
    Whole(Arc<Whole>),
    Part(Option<Laid>),
}

struct Kept {
    key: PictureKey,
    picture: Picture,
}

/// The pictures of the program's screens, which its model owns: what each screen shows, the pictures made last for
/// any screen (at most eight besides those shown, least recently used first) and those being made.
#[derive(Default)]
pub struct ScreenPictures {
    screens: Vec<(WindowKey, Shown)>,
    kept: Vec<Kept>,
    /// The bands of whole pictures being made, as they come in.
    building: Vec<(PictureKey, Vec<Option<Laid>>)>,
    pending: Vec<PictureKey>,
    made: u64,
}

impl fmt::Debug for ScreenPictures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenPictures")
            .field("screens", &self.screens)
            .field("kept", &self.kept.len())
            .field("pending", &self.pending)
            .finish()
    }
}

impl ScreenPictures {
    pub fn new() -> ScreenPictures {
        ScreenPictures::default()
    }

    /// After each update: what every screen of the program shows now, by the key of its window, at the device scale of
    /// its output. A screen not named is forgotten. Gives the commands that make the pictures the screens want and do
    /// not have (none for a picture kept or being made, so a second screen of the same size, scale and wallpaper takes
    /// the first one's); each picture comes back as a [`Made`], which \`map\` makes a message of. The glass letters are
    /// shaped in \`env\`'s text system.
    pub fn want<M: Send + 'static>(
        &mut self,
        env: &Environment,
        screens: Vec<(WindowKey, Wants)>,
        map: fn(Made) -> M,
    ) -> Vec<Command<M>> {
        self.screens
            .retain(|(key, _)| screens.iter().any(|(named, _)| named == key));
        for (key, wants) in screens {
            match self.screens.iter_mut().find(|(k, _)| *k == key) {
                Some((_, shown)) => shown.wants = Some(wants),
                None => self.screens.push((
                    key,
                    Shown {
                        wants: Some(wants),
                        ..NOTHING.clone()
                    },
                )),
            }
        }
        self.plan(env, map)
    }

    /// A picture made on the CPU pool: the screens that want it show it. Gives the commands that make what the screens
    /// want next (the next frame of the dimming coming in, the glass letters from their glyphs).
    pub fn made<M: Send + 'static>(
        &mut self,
        env: &Environment,
        made: Made,
        map: fn(Made) -> M,
    ) -> Vec<Command<M>> {
        match made.0 {
            Arrival::Band { key, index, laid } => {
                let Some(at) = self.building.iter().position(|(k, _)| *k == key) else {
                    return Vec::new();
                };
                if let Some(slot) = self.building[at].1.get_mut(index) {
                    *slot = Some(laid);
                }
                if self.building[at].1.iter().any(Option::is_none) {
                    return Vec::new();
                }
                let (_, bands) = self.building.remove(at);
                self.pending.retain(|k| *k != key);
                let whole = Arc::new(Whole {
                    key,
                    bands: bands.into_iter().flatten().collect(),
                });
                if matches!(key.kind, Kind::Undimmed | Kind::Dimmed) {
                    self.made += 1;
                }
                self.keep(key, Picture::Whole(Arc::clone(&whole)));
                self.show_progress(&whole);
            }
            Arrival::Part { key, laid } => {
                if !self.pending.contains(&key) {
                    return Vec::new();
                }
                self.pending.retain(|k| *k != key);
                self.keep(key, Picture::Part(laid));
            }
            Arrival::Coverage {
                key,
                source,
                coverage,
                bound,
            } => {
                if !self.pending.contains(&key) {
                    return Vec::new();
                }
                let scale = f64::from_bits(key.scale);
                let (width, height) = (key.width, key.height);
                return vec![task::cpu(
                    move || {
                        let coverage = coverage?;
                        let (region, alpha) = coverage_within(&coverage, bound)?;
                        let pixels =
                            wallpaper::glass_letters(&source, width, height, scale, region, &alpha);
                        (!pixels.is_empty()).then(|| part(region, pixels))
                    },
                    move |image: Option<(ImageResource, [u32; 4])>| {
                        map(Made(Arrival::Part {
                            key,
                            laid: image.map(|(image, region)| laid(image, region, scale)),
                        }))
                    },
                )];
            }
        }
        self.plan(env, map)
    }

    /// What the screen of window \`key\` shows, for its view; nothing for a screen not named to [`ScreenPictures::want`].
    pub fn shown(&self, key: &WindowKey) -> &Shown {
        self.screens
            .iter()
            .find(|(k, _)| k == key)
            .map_or(&NOTHING, |(_, shown)| shown)
    }

    /// Whether every picture the screen of window \`key\` wants is in ([`Shown::ready`]): a lock surface's first frame waits
    /// for it.
    pub fn ready(&self, key: &WindowKey) -> bool {
        self.shown(key).ready()
    }

    /// How many pictures of a whole screen were made from their wallpaper: once per screen size, scale and wallpaper,
    /// and once more as the dimming has come in. What a frame of a typed character, a hover or a fade draws leaves it
    /// as it is.
    pub fn pictures_made(&self) -> u64 {
        self.made
    }

    /// How many pictures are kept besides those the screens show: at most eight.
    pub fn kept(&self) -> usize {
        self.kept.len()
    }

    /// Drop every picture, shown or kept, and forget the screens (the lock screen ends, the greeter starts a session):
    /// a screen's memory is given back. Pictures still being made are dropped as they come in.
    pub fn forget(&mut self) {
        *self = ScreenPictures {
            made: self.made,
            ..ScreenPictures::default()
        };
    }

    /// Every screen shown what it wants that is kept, and the commands that make what is neither kept nor being made.
    fn plan<M: Send + 'static>(
        &mut self,
        env: &Environment,
        map: fn(Made) -> M,
    ) -> Vec<Command<M>> {
        let mut out = Vec::new();
        for i in 0..self.screens.len() {
            let Some(wants) = self.screens[i].1.wants.clone() else {
                continue;
            };
            let t = wants.targets();
            self.plan_ground(i, &wants, &t, map, &mut out);
            self.plan_materials(i, &wants, &t, map, &mut out);
            if let Some(key) = t.glass {
                let shown = self.screens[i].1.glass.as_ref().map(|(k, _)| *k);
                if shown != Some(key) {
                    if let Some(found) = self.part(key) {
                        self.screens[i].1.glass = Some((key, found));
                    } else if !self.pending.contains(&key) {
                        if let Some(glass) = &wants.glass {
                            out.extend(self.make_glass(env, &wants, glass, key, map));
                        }
                    }
                }
            } else {
                self.screens[i].1.glass = None;
            }
        }
        out
    }

    fn plan_ground<M: Send + 'static>(
        &mut self,
        i: usize,
        wants: &Wants,
        t: &Targets,
        map: fn(Made) -> M,
        out: &mut Vec<Command<M>>,
    ) {
        let shown = self.screens[i].1.ground.as_ref().map(|g| g.key);
        if shown == Some(t.ground) {
            return;
        }
        if let Some(whole) = self.whole(t.ground) {
            self.screens[i].1.ground = Some(whole);
            return;
        }
        let Kind::Frame(bits) = t.ground.kind else {
            if !self.pending.contains(&t.ground) {
                let dim = if t.ground.kind == Kind::Dimmed {
                    1.0
                } else {
                    0.0
                };
                out.extend(self.make_whole(wants, t.ground, dim, map));
            }
            return;
        };
        // a frame of the dimming coming in is made from the picture before it
        let undimmed = t.ground.with(Kind::Undimmed);
        match self.whole(undimmed) {
            Some(whole) => {
                if self.screens[i].1.ground.is_none() {
                    self.screens[i].1.ground = Some(Arc::clone(&whole));
                }
                // one frame of a screen at a time: the next is made for the dimming as it is once this one is in
                let framing = self
                    .pending
                    .iter()
                    .any(|k| matches!(k.kind, Kind::Frame(_)) && k.same_screen(&t.ground));
                if !framing {
                    out.extend(self.make_frame(&whole, t.ground, f32::from_bits(bits), map));
                }
            }
            None => {
                if !self.pending.contains(&undimmed) {
                    out.extend(self.make_whole(wants, undimmed, 0.0, map));
                }
            }
        }
    }

    fn plan_materials<M: Send + 'static>(
        &mut self,
        i: usize,
        wants: &Wants,
        t: &Targets,
        map: fn(Made) -> M,
        out: &mut Vec<Command<M>>,
    ) {
        // the strips the screen no longer has go; one whose material is being made shows the one it had
        self.screens[i]
            .1
            .materials
            .retain(|(strip, _, _)| t.strips.iter().any(|(s, _)| s == strip));
        for &(strip, key) in &t.strips {
            let shown = self.screens[i]
                .1
                .materials
                .iter()
                .position(|(s, _, _)| *s == strip);
            if shown.is_some_and(|at| self.screens[i].1.materials[at].1 == key) {
                continue;
            }
            let found = match key {
                None => Some(None),
                Some(key) => self.part(key),
            };
            match (found, shown) {
                (Some(laid), Some(at)) => self.screens[i].1.materials[at] = (strip, key, laid),
                (Some(laid), None) => self.screens[i].1.materials.push((strip, key, laid)),
                (None, _) => {
                    if let Some(key) = key.filter(|key| !self.pending.contains(key)) {
                        out.push(self.make_material(wants, key, map));
                    }
                }
            }
        }
        match t.notices {
            None => self.screens[i].1.notices = None,
            Some(key) => {
                if self.screens[i].1.notices.as_ref().map(|(k, _)| *k) == Some(key) {
                    return;
                }
                let found = match key {
                    None => Some(None),
                    Some(key) => self.part(key),
                };
                match found {
                    Some(laid) => self.screens[i].1.notices = Some((key, laid)),
                    None => {
                        if let Some(key) = key.filter(|key| !self.pending.contains(key)) {
                            out.push(self.make_material(wants, key, map));
                        }
                    }
                }
            }
        }
    }

    /// The whole picture of \`key\` if it is kept or shown, now the latest used.
    fn whole(&mut self, key: PictureKey) -> Option<Arc<Whole>> {
        if let Some(Picture::Whole(whole)) = self.touch(key) {
            return Some(whole);
        }
        self.screens
            .iter()
            .filter_map(|(_, shown)| shown.ground.as_ref())
            .find(|g| g.key == key)
            .cloned()
    }

    /// The material or glass letters of \`key\` if they are kept or shown (\`Some(None)\`: a picture of nothing).
    fn part(&mut self, key: PictureKey) -> Option<Option<Laid>> {
        if let Some(Picture::Part(laid)) = self.touch(key) {
            return Some(laid);
        }
        self.screens.iter().find_map(|(_, shown)| {
            let materials = shown.materials.iter().map(|(_, k, laid)| (*k, laid));
            let notices = shown.notices.iter().map(|(k, laid)| (*k, laid));
            let glass = shown.glass.iter().map(|(k, laid)| (Some(*k), laid));
            materials
                .chain(notices)
                .chain(glass)
                .find(|(k, _)| *k == Some(key))
                .map(|(_, laid)| laid.clone())
        })
    }

    /// The kept picture of \`key\`, now the latest used.
    fn touch(&mut self, key: PictureKey) -> Option<Picture> {
        let at = self.kept.iter().position(|k| k.key == key)?;
        let entry = self.kept.remove(at);
        let picture = entry.picture.clone();
        self.kept.push(entry);
        Some(picture)
    }

    /// Keeps \`picture\` as the latest used, dropping what it replaces and the one used longest ago past
    /// [`KEPT_PICTURES`].
    fn keep(&mut self, key: PictureKey, picture: Picture) {
        let screen = |k: &Kept| k.key.same_screen(&key);
        // a frame of the dimming replaces the one before; the screen with its dimming in needs neither its frames nor
        // its undimmed picture; the time of the minute before is not shown again
        match key.kind {
            Kind::Frame(_) => self
                .kept
                .retain(|k| !(screen(k) && matches!(k.key.kind, Kind::Frame(_)))),
            Kind::Dimmed => self
                .kept
                .retain(|k| !(screen(k) && matches!(k.key.kind, Kind::Frame(_) | Kind::Undimmed))),
            Kind::Glass(_) => self
                .kept
                .retain(|k| !(screen(k) && matches!(k.key.kind, Kind::Glass(_)))),
            Kind::Undimmed | Kind::Material(_) => {}
        }
        self.kept.retain(|k| k.key != key);
        self.kept.push(Kept { key, picture });
        while self.kept.len() > KEPT_PICTURES {
            self.kept.remove(0);
        }
    }

    /// A whole picture just made shown on each screen of its wallpaper, size and scale that does not have the one it
    /// wants: the picture it wants, or the latest frame of its dimming coming in, or the screen's first picture.
    fn show_progress(&mut self, whole: &Arc<Whole>) {
        for (_, shown) in &mut self.screens {
            let Some(wants) = &shown.wants else {
                continue;
            };
            let target = wants.targets().ground;
            let current = shown.ground.as_ref().map(|g| g.key);
            if !target.same_screen(&whole.key) || current == Some(target) {
                continue;
            }
            let closer = whole.key == target
                || current.is_none_or(|c| !c.same_screen(&target))
                || (whole.key.kind != Kind::Undimmed && target.kind != Kind::Undimmed);
            if closer {
                shown.ground = Some(Arc::clone(whole));
            }
        }
    }

    /// The commands that make the whole picture \`key\` of \`wants\`' wallpaper with the dimming at \`dim\`, a band a task.
    fn make_whole<M: Send + 'static>(
        &mut self,
        wants: &Wants,
        key: PictureKey,
        dim: f32,
        map: fn(Made) -> M,
    ) -> Vec<Command<M>> {
        let (width, height) = (key.width, key.height);
        let scale = f64::from_bits(key.scale);
        let rows = bands(wants.size.height, height, scale);
        self.pending.push(key);
        self.building.push((key, vec![None; rows.len()]));
        rows.into_iter()
            .enumerate()
            .map(|(index, (y0, y1))| {
                let source = wants.source.clone();
                task::cpu(
                    move || {
                        band(width, y0, y1, |out| {
                            wallpaper::render_rows_into(
                                &source,
                                width,
                                height,
                                scale,
                                dim,
                                y0..y1,
                                out,
                            );
                        })
                    },
                    move |image| {
                        map(Made(Arrival::Band {
                            key,
                            index,
                            laid: laid(image, [0, y0, width, y1], scale),
                        }))
                    },
                )
            })
            .collect()
    }

    /// The commands that make the frame \`key\` of the dimming coming in at \`dim\` from the picture before it, a band a
    /// task.
    fn make_frame<M: Send + 'static>(
        &mut self,
        undimmed: &Arc<Whole>,
        key: PictureKey,
        dim: f32,
        map: fn(Made) -> M,
    ) -> Vec<Command<M>> {
        let (width, height) = (key.width, key.height);
        let scale = f64::from_bits(key.scale);
        self.pending.push(key);
        self.building.push((key, vec![None; undimmed.bands.len()]));
        undimmed
            .bands
            .iter()
            .enumerate()
            .map(|(index, from)| {
                let [_, y0, _, y1] = from.region;
                let pixels = Arc::clone(&from.image.pixels);
                task::cpu(
                    move || {
                        band(width, y0, y1, |out| {
                            wallpaper::dim_rows_into(&pixels, width, height, scale, dim, y0, out);
                        })
                    },
                    move |image| {
                        map(Made(Arrival::Band {
                            key,
                            index,
                            laid: laid(image, [0, y0, width, y1], scale),
                        }))
                    },
                )
            })
            .collect()
    }

    /// The command that makes the control material \`key\` of \`wants\`' wallpaper.
    fn make_material<M: Send + 'static>(
        &mut self,
        wants: &Wants,
        key: PictureKey,
        map: fn(Made) -> M,
    ) -> Command<M> {
        let Kind::Material(region) = key.kind else {
            unreachable!("a material's key")
        };
        let (width, height) = (key.width, key.height);
        let scale = f64::from_bits(key.scale);
        let source = wants.source.clone();
        self.pending.push(key);
        task::cpu(
            move || {
                let pixels = wallpaper::frosted(&source, width, height, scale, region);
                (!pixels.is_empty()).then(|| part(region, pixels))
            },
            move |image| {
                map(Made(Arrival::Part {
                    key,
                    laid: image.map(|(image, region)| laid(image, region, scale)),
                }))
            },
        )
    }

    /// The command that draws the glyphs of the glass letters \`glass\` (shaped in \`env\`'s text system) on the CPU pool,
    /// from which [`ScreenPictures::made`] then makes the letters; none without glyphs.
    fn make_glass<M: Send + 'static>(
        &mut self,
        env: &Environment,
        wants: &Wants,
        glass: &Glass,
        key: PictureKey,
        map: fn(Made) -> M,
    ) -> Option<Command<M>> {
        let scale = f64::from_bits(key.scale);
        let bound = glass.bound(key.width, key.height, scale);
        if bound[0] >= bound[2] || bound[1] >= bound[3] {
            self.keep(key, Picture::Part(None));
            return None;
        }
        let label = super::widgets::Label {
            color: Some(WHITE),
            ..label_of(
                &glass.text,
                glass.size,
                glass.weight,
                glass.rect,
                glass.ascent,
            )
        };
        let Some(layout) = label.laid_out(&env.text) else {
            self.keep(key, Picture::Part(None));
            return None;
        };
        // each glyph where the renderer places it on the screen: the picture starts at the screen's top left corner, so
        // the glyphs fall on the very device pixels they cover there
        let at = glass.at();
        let first = layout.lines().first().map_or(0.0, |line| line.baseline);
        let ops: Vec<PaintOp> = layout
            .lines()
            .iter()
            .flat_map(|line| &line.runs)
            .map(|run| PaintOp::Glyphs {
                origin: Point::new(
                    at.x + run.origin.x,
                    at.y + glass.ascent - first + run.origin.y,
                ),
                run: run.run.clone(),
                color: WHITE,
            })
            .collect();
        let size = Size::new(f64::from(bound[2]) / scale, f64::from(bound[3]) / scale);
        let source = wants.source.clone();
        self.pending.push(key);
        Some(render::picture(env, ops, size, scale, move |coverage| {
            map(Made(Arrival::Coverage {
                key,
                source,
                coverage,
                bound,
            }))
        }))
    }
}

/// The bands of rows a screen \`points\` tall, \`height\` device pixels at \`scale\`, is made in: one a row of
/// [`PICTURE_TILE`] squares, with the device rows the squares of that row cover (the last to the screen's bottom).
fn bands(points: f64, height: u32, scale: f64) -> Vec<(u32, u32)> {
    let edge = |v: f64| (v * scale).round().clamp(0.0, f64::from(height)) as u32;
    let count = (points.max(0.0) / PICTURE_TILE).ceil().max(1.0) as usize;
    (0..count)
        .map(|row| {
            let top = edge(row as f64 * PICTURE_TILE);
            let bottom = if row + 1 == count {
                height
            } else {
                edge((row + 1) as f64 * PICTURE_TILE)
            };
            (top, bottom)
        })
        .filter(|(top, bottom)| top < bottom)
        .collect()
}

/// Opaque premultiplied pixels of the rows \`y0..y1\` of a picture \`width\` pixels wide, made in place by \`fill\`.
fn band(width: u32, y0: u32, y1: u32, fill: impl FnOnce(&mut [u8])) -> ImageResource {
    let height = y1.saturating_sub(y0);
    let len = width as usize * height as usize * 4;
    // memory the system hands over zeroed, untouched until \`fill\` writes it: a whole screen of it is not set to zero
    // first, and its pages are first touched by the thread that fills them
    let mut pixels: Arc<[u8]> = zeroed_pixels(len);
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

/// Premultiplied \`pixels\` of \`region\` (device pixels \`[x0, y0, x1, y1]\`) as a picture, and the region.
fn part(region: [u32; 4], pixels: Vec<u8>) -> (ImageResource, [u32; 4]) {
    let width = region[2] - region[0];
    (
        ImageResource {
            revision: 1,
            width,
            height: region[3] - region[1],
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels),
        },
        region,
    )
}

/// \`image\` laid over \`region\` of a screen at \`scale\`, under an id of its own.
fn laid(image: ImageResource, region: [u32; 4], scale: f64) -> Laid {
    Laid {
        id: next_resource_id(),
        image,
        region,
        scale,
    }
}

/// The glyphs' coverage within \`bound\` of \`picture\` (white glyphs on clear over the screen from its top left corner),
/// cut to the pixels it covers: the region of the screen they cover and its coverage, a byte a pixel row by row.
/// \`None\` where no glyph covers any.
fn coverage_within(picture: &ImageResource, bound: [u32; 4]) -> Option<([u32; 4], Vec<u8>)> {
    let [bx0, by0, bx1, by1] = [
        bound[0],
        bound[1],
        bound[2].min(picture.width),
        bound[3].min(picture.height),
    ];
    let alpha =
        |x: u32, y: u32| picture.pixels[y as usize * picture.stride as usize + x as usize * 4 + 3];
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in by0..by1 {
        for x in bx0..bx1 {
            if alpha(x, y) != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let mut coverage = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
    for y in y0..y1 {
        coverage.extend((x0..x1).map(|x| alpha(x, y)));
    }
    Some(([x0, y0, x1, y1], coverage))
}
