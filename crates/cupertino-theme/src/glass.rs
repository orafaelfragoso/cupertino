//! Glass material recipes: every value of the background pass (blur, face colour, refraction, bleed, light and
//! shadows) and the foreground colour matrix, for a [`GlassMode`] (appearance, variant, modifiers and accessibility
//! options) and the size of the shape it covers.
//!
//! A recipe is plain data. `cupertino-glass` turns it into GPU uniforms and holds the CPU model of each stage;
//! the README of that crate describes what every value does.
//!
//! * [`GlassVariant`] names the glass variants of the system, each built on the regular or the clear glass
//!   ([`GlassBase`]) with stages of its own; [`GlassModifiers`] say the size it resolves at, whether its window is
//!   active, and whether it steps back or leaves its shadow to its surface. [`GlassStyle`] is the legacy style of the
//!   wire and the toolkit, mapped onto a variant ([`GlassStyle::variant`]).
//! * Values are recorded from the system where the recipes record them, and fitted to captures where nothing is
//!   recorded; the comments say which.
//! * Size: several values follow the short side of the shape ([`GlassParams::new`]) or the size the variant resolves
//!   at ([`GlassSize`]), so a small control bends and bleeds less than a large panel.
//! * Accessibility selects recipes of its own, it does not turn the glass off: Reduce Transparency trades refraction for
//!   a wide blur and a denser face, Increase Contrast keeps the optics with darker or lighter colours, both together
//!   take the optics of the first and the colours of both, and Reduce Motion flattens the refraction and widens the
//!   blur. Each base has its own stages; the Dock and the control centre add theirs.
//! * The legibility amount ([`GlassMode::density`]) fills the glass with more of its own blurred backdrop.

/// The scale the backdrop of a glass container is captured at, relative to the output's pixels, unless its recipe
/// says otherwise ([`GlassParams::capture_scale`]).
pub const GLASS_BACKDROP_SCALE: f32 = 0.5;
/// The capture scale of a notification card's glass (fitted to captures).
pub const CARD_CAPTURE_SCALE: f32 = 0.25;
/// The capture scale of the controls' glass, which does not blur: the output's own.
pub const TRACK_CAPTURE_SCALE: f32 = 1.0;

/// Most blurred copies of the backdrop one container mixes: one per distinct blur of its shapes (each shape's Gaussian
/// follows its size), up to this many; a shape whose blur lies between two of them mixes the two.
pub const BLUR_LEVELS: usize = 4;

/// The Gaussian deviation of a glass blur, in points per point of its radius scaled by the first level's opacity: a
/// regular control (radius 5 at opacity 0.4) blurs with a deviation of 4 points, a large regular panel (opacity 0.8)
/// with 8.
pub const BLUR_SIGMA_PER_RADIUS: f32 = 2.0;
/// The Gaussian deviation of the blur fill, in points per point of its radius.
pub const FILL_SIGMA_PER_RADIUS: f32 = 2.0;
/// The widest deviation a glass blur takes, in points: past it the face reads as the average colour around it anyway,
/// and the backdrop it would read grows with it.
pub const BLUR_SIGMA_MAX: f32 = 100.0;

/// The short side, in points, below which a glass control counts as small: its foreground follows what is behind it
/// (light text over a dark backdrop, dark over a light one), while larger bars and panels keep their appearance.
pub const SMALL_CONTROL_SIDE: f32 = 64.0;

/// A 4×5 colour matrix, row-major: `out.r = m[0]·r + m[1]·g + m[2]·b + m[3]·a + m[4]`, and so on for g, b and a.
pub type ColorMatrix = [f32; 20];

/// The legacy glass styles, as the wire and the toolkit name them. Each draws as a system variant with modifiers
/// ([`GlassStyle::variant`]); the variant's recipe is the style's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassStyle {
    /// Controls, bars, menus and panels with text on them: [`GlassVariant::Regular`].
    #[default]
    Regular,
    /// Controls over photos and video: [`GlassVariant::Clear`].
    Clear,
    /// Large floating panels (the launcher): [`GlassVariant::SearchResults`].
    LargePanel,
    /// A floating bar of icons (the dock): [`GlassVariant::Dock`].
    Bar,
    /// A dense floating surface with text on it (the dock's stack grid, a popover with a pointer):
    /// [`GlassVariant::CartouchePopover`] at the medium size.
    Dense,
    /// The tiles of a panel of modules (the control panel): [`GlassVariant::ControlCenter`].
    Module,
    /// A notification card: [`GlassVariant::NotificationCenter`] at the medium size.
    Card,
    /// A level's track and a held knob's lens: [`GlassVariant::Slider`].
    Track,
    /// A small popover over a floating bar (a tile's name, a tile's menu): [`GlassVariant::CartouchePopover`] at the
    /// minimum size.
    Popover,
    /// A wide floating panel of large icons (the application switcher): [`GlassVariant::AppSwitcher`].
    Switcher,
    /// A window's sidebar: [`GlassVariant::AbuttedSidebar`].
    Sidebar,
    /// A panel of modules that steps back: [`GlassVariant::ControlCenter`], subdued.
    SubduedModule,
    /// A small card of content over the desktop: the nearest variant, [`GlassVariant::NotificationCenter`] at its own
    /// size.
    Snippet,
    /// Clear glass veiled black towards its bottom: the nearest variant, [`GlassVariant::Clear`], without the veil.
    Veiled,
    /// A window's toolbar controls: [`GlassVariant::ToolbarItem`].
    Toolbar,
    /// A menu: [`GlassVariant::Menu`], whose window shadow the popup draws apart.
    Menu,
}

/// Everything besides the shape's size that selects a recipe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlassMode {
    pub dark: bool,
    /// The variant drawn.
    pub variant: GlassVariant,
    /// What changes how the variant resolves.
    pub modifiers: GlassModifiers,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
    /// How much denser the person asked every material to be, in percent (0 leaves the recipe as it is): the
    /// legibility amount `0.5 + density / 200` ([`legibility_amount`]).
    pub density: u8,
}

/// The blur of the backdrop and the blur fill laid over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBlur {
    /// Blur radius in points; with the first level's opacity it sets the Gaussian ([`GlassParams::blur_sigma`]).
    pub radius: f32,
    /// Opacity of the levels: the first scales the blur radius (a small control blurs less than a large panel of the
    /// same recipe); the others leave the blur as it is.
    pub level_opacity: [f32; BLUR_LEVELS],
    /// Blur radius in points of the fill blended over the blurred backdrop.
    pub fill_radius: f32,
    /// Opacity of the fill in darken blending: keeps the darker of the two, which drops thin bright detail.
    pub fill_darken: f32,
    /// Opacity of the fill in normal blending.
    pub fill_normal: f32,
    /// Opacity of the fill in lighten blending: keeps the lighter of the two, which drops thin dark detail.
    pub fill_lighten: f32,
}

/// The face colour: how the blurred backdrop is toned before the edge effects. Every step works on encoded values, in
/// this order: saturation, the luma compression of `max_luma`, the levels of `black` and `white`, then the fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassFace {
    /// Saturation about the Rec. 709 luma of the encoded colour (1 leaves the colour).
    pub saturation: f32,
    /// The white level: the face loses `(1 − white) · L` on every channel, `L` its luma after the compression, so a
    /// value under 1 dims bright faces more than dark ones (over 1 it brightens them).
    pub white: f32,
    /// The black level: the face gains `black · (1 − L)` on every channel, so dark faces are lifted most.
    pub black: f32,
    /// Largest encoded channel value of the face.
    pub clamp: f32,
    /// The luma compression: a colour `c` of luma `L` becomes `c − (1 − max_luma) · L · (0.75 c + 0.25 L)`, so a grey
    /// `x` becomes `x − (1 − max_luma) · x²` and white comes out at `max_luma`.
    pub max_luma: f32,
    /// Straight sRGB fill laid over the face.
    pub fill: [f32; 4],
}

/// Refraction at the inner edge of the shape, and the bend of the backdrop just outside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassRefraction {
    /// Strength in points: a pixel at the edge reads the face [`REFRACTION_EDGE_SHARE`] of it along the outward normal,
    /// less deeper in the band ([`refraction_profile`]); negative values read it further inside, as a convex rim does.
    pub amount: f32,
    /// Width in points of the band inside the edge that bends.
    pub height: f32,
    /// Strength in points of the bend outside the edge: a pixel `d` outside the outline reads the backdrop
    /// `outer_amount · g(d / outer_height)` points along the outward normal (`g` the inner band's
    /// [`refraction_profile`]), so a negative strength draws what lies next to the shape towards it.
    pub outer_amount: f32,
    /// Width in points of the band outside the edge that bends.
    pub outer_height: f32,
    /// How much of the bent backdrop is laid over what lies outside the edge, fading out across the band: none at
    /// rest.
    pub outer_opacity: f32,
}

/// The backdrop just outside the shape bleeding into the inner edge: inside a band `height` deep along the edge, the
/// backdrop read `amount · (1 − t)` points outside the pixel (`t` the depth into the band, 0 at the edge), blurred by a
/// Gaussian of deviation `blur`, through its own saturation and levels, blends into the face at `opacity · (1 − t)²`,
/// so the rim picks up the colour of what surrounds it. Lengths are points for the shape's size;
/// [`GlassBleed::normalized`] sets them as fractions of its short side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBleed {
    /// How far outside the pixel the bleed reads the backdrop at the edge, in points; none at the band's inner side.
    pub amount: f32,
    /// Width in points of the band it fades over.
    pub height: f32,
    /// The Gaussian deviation in points of the blur of the backdrop it reads.
    pub blur: f32,
    /// The white level of the bleed's colour: it loses `(1 − white) · L` on every channel, `L` its luma.
    pub white: f32,
    /// The black level: the bleed's colour gains `black · (1 − L)` on every channel.
    pub black: f32,
    /// Saturation about the Rec. 709 luma of the bleed's encoded colour (1 leaves it).
    pub saturation: f32,
    /// Darken blending, the smaller of each channel (light glass), instead of normal blending (dark glass).
    pub darken: bool,
    /// How much of the bleed shows: none at rest.
    pub opacity: f32,
}

impl GlassBleed {
    /// The bleed with its band, its reach and its blur set as fractions of `short_side` (points), the blur at most
    /// `max_blur` points: the bleed of a shape of that size from the size-free values of a recipe.
    pub fn normalized(
        self,
        short_side: f32,
        height: f32,
        amount: f32,
        blur: f32,
        max_blur: f32,
    ) -> Self {
        let s = if short_side.is_finite() {
            short_side.max(0.0)
        } else {
            0.0
        };
        Self {
            height: height * s,
            amount: amount * s,
            blur: (blur * s).min(max_blur).max(0.0),
            ..self
        }
    }
}

/// The key and fill light along the edge, added to the face as white light: the arc of edge facing the key light, and
/// the opposite one facing the fill light at `fill` of its strength, each lit at `amount` in a band `height` deep that
/// fades out along a smoothstep; under it the diffuse glow the same lights cast, a band `height · diffuse.height_scale`
/// deep fading as the square of what is left of it, over arcs `diffuse.spread_scale` as wide, at
/// `amount · diffuse.amount_scale`. An arc lights `1 − smoothstep(0, spread, a)`, `a` the angle between the edge's
/// outward normal and its light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassSpecular {
    /// Strength of the key light (0 for none).
    pub amount: f32,
    /// Half-width in radians of each arc: how far from its light's direction the arc reaches.
    pub spread: f32,
    /// Depth in points of the band.
    pub height: f32,
    /// Direction the key light comes from, in radians, counter-clockwise from the right with y up (π/2 is the top);
    /// the fill light comes from the opposite direction.
    pub angle: f32,
    /// Strength of the fill light per unit of the key's.
    pub fill: f32,
    /// The diffuse glow, in multiples of the band's depth, the arcs' half-width and the strength.
    pub diffuse: GlowDiffuse,
}

impl GlassSpecular {
    /// No key or fill light.
    pub const NONE: Self = Self {
        amount: 0.0,
        spread: 0.0,
        height: 0.0,
        angle: HIGHLIGHT_ANGLE,
        fill: 1.0,
        diffuse: GlowDiffuse::NONE,
    };
}

/// The outline a control draws round its glass: a black line of `opacity`, `width` points wide, centred on the edge and
/// laid under the key and fill light, which hide it down to [`BORDER_LIT_SHARE`] where they fall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassBorder {
    /// Opacity of the line (0 for none).
    pub opacity: f32,
    /// Width in points.
    pub width: f32,
}

impl GlassBorder {
    /// No outline.
    pub const NONE: Self = Self {
        opacity: 0.0,
        width: 0.0,
    };
}

/// How much of the control outline ([`GlassBorder`]) shows where the key or fill light falls on it fully.
pub const BORDER_LIT_SHARE: f32 = 0.2;

/// The rim along the edge, and the soft band of light some surfaces lay under their top and bottom edges.
///
/// The rim shifts every channel of the face by `amount · color_bias` (a negative bias darkens), scaled by the face's
/// brightness and by the direction of the edge, strongest where the edge runs diagonally, in a band
/// [`RIM_DEPTH_PER_HEIGHT`] times `height` deep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassHighlight {
    /// Strength of the rim.
    pub amount: f32,
    /// Direction the soft band's light comes from, in radians, counter-clockwise from the right with y up (π/2 is the
    /// top).
    pub angle: f32,
    /// Angular width of each arc the soft band lights, in radians; an arc fades to nothing three quarters of it from its
    /// end of the axis.
    pub spread: f32,
    /// The rim's depth in points, as a share of [`RIM_DEPTH_PER_HEIGHT`].
    pub height: f32,
    /// Unused by the rim; kept with the recipe.
    pub offset: f32,
    /// The colour shift of the rim per unit of strength: negative darkens. The soft band's far end of the axis is lit at
    /// `1 + color_bias` of its near end.
    pub color_bias: f32,
    /// Exponent on the soft band's arc falloff.
    pub curvature: f32,
    /// A soft band of light along the arcs: its strength (0 for none), its half-width and where its middle lies
    /// relative to the edge, in points.
    pub glow_amount: f32,
    pub glow_height: f32,
    pub glow_offset: f32,
    /// How strongly the soft band lights the far end of its axis, as a share of the near end.
    pub glow_fill: f32,
    /// A second, wider and fainter band of the same light under the soft band: its half-width, the angular width of its
    /// arcs and its strength as multiples of the soft band's (all 0 for none).
    pub diffuse: GlowDiffuse,
    /// Colour of the soft band's light, from the face colour under it.
    pub matrix: ColorMatrix,
}

/// The wider, fainter band under the soft band of [`GlassHighlight`], in multiples of the soft band's values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlowDiffuse {
    /// Half-width per unit of the soft band's half-width.
    pub height_scale: f32,
    /// Angular width of its arcs per unit of the soft band's.
    pub spread_scale: f32,
    /// Strength per unit of the soft band's strength.
    pub amount_scale: f32,
}

impl GlowDiffuse {
    /// No second band.
    pub const NONE: Self = Self {
        height_scale: 0.0,
        spread_scale: 0.0,
        amount_scale: 0.0,
    };
}

/// How deep the rim reaches into the shape, per point of its `height`.
pub const RIM_DEPTH_PER_HEIGHT: f32 = 1.7;
/// How strongly the rim shows by the luma of the face under it `(luma, response)`; linear between: nothing on black,
/// most a little above the middle.
pub const RIM_BRIGHTNESS_RESPONSE: [(f32, f32); 5] = [
    (0.0, 0.0),
    (0.25, 0.63),
    (0.5, 1.0),
    (0.75, 1.13),
    (1.0, 1.0),
];
/// The share of the rim along straight edges (horizontal and vertical); the rest grows with `sin²(2φ)` to the full rim
/// where the edge runs diagonally.
pub const RIM_STRAIGHT_SHARE: f32 = 0.15;

/// The rim's response to a face of luma `luma` ([`RIM_BRIGHTNESS_RESPONSE`]).
pub fn rim_brightness(luma: f32) -> f32 {
    by_side(&RIM_BRIGHTNESS_RESPONSE, luma.clamp(0.0, 1.0))
}

/// The rim's response to an edge whose outward normal points at `angle` radians: [`RIM_STRAIGHT_SHARE`] on straight
/// edges, all of it on diagonal ones.
pub fn rim_direction(angle: f32) -> f32 {
    let s = (2.0 * angle).sin();
    RIM_STRAIGHT_SHARE + (1.0 - RIM_STRAIGHT_SHARE) * s * s
}

/// The drop shadow around the shape and the ring shadow inside it. Both are drawn at their opacity, which is 0 at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassShadow {
    /// Offset in points (y down).
    pub offset: [f32; 2],
    /// Strength in percent of the drop shadow.
    pub amount: f32,
    /// Blur radius in points of the drop shadow.
    pub height: f32,
    /// Straight sRGB colour of the drop shadow.
    pub fill: [f32; 4],
    pub saturation: f32,
    pub opacity: f32,
    /// Gaussian deviation in points of the ring shadow: a stroke along the inside of the outline that darkens the face.
    pub ring_blur: f32,
    /// Width in points of the ring.
    pub ring_stroke: f32,
    /// How far up the ring is moved, in points.
    pub ring_offset: f32,
    /// How much the ring darkens the face: none at rest.
    pub ring_opacity: f32,
    /// A hairline just outside the shape, in the drop shadow's colour: its opacity (0 for none) and its width in points.
    pub outline_opacity: f32,
    pub outline_width: f32,
    /// The edge line: a line one device pixel wide just outside the outline, black at this alpha times the rim's
    /// response to the brightness of what lies there ([`rim_brightness`]), so the glass keeps an outline over light
    /// content and draws none over black. 0 for none.
    pub edge_line: f32,
    /// The angular width, in radians, of the arcs facing left and right that keep the edge line: a raised cosine of
    /// the outline's direction, so a straight side keeps all of it, a top or bottom edge none, and a corner fades from
    /// one to the other. 0 keeps the line all round.
    pub edge_line_sides: f32,
    /// Whether the edge line keeps its strength over any backdrop rather than following the rim's response to its
    /// brightness, so it reads as dark over a dark scene as over a bright one.
    pub edge_line_even: bool,
}

/// The complete background recipe of one glass shape.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassParams {
    pub blur: GlassBlur,
    pub face: GlassFace,
    pub refraction: GlassRefraction,
    pub bleed: GlassBleed,
    pub highlight: GlassHighlight,
    pub shadow: GlassShadow,
    /// The scale the backdrop is captured at, relative to the output's pixels, before it is blurred: a coarser capture
    /// softens fine detail on its own, a full one keeps it sharp where nothing blurs it.
    pub capture_scale: f32,
    /// A black veil over the face, its opacity at the top and at the bottom of the shapes' bounds, linear between and
    /// laid after the rim; `[0, 0]` for none.
    pub veil: [f32; 2],
    /// The key and fill light added along the edge ([`GlassSpecular`]): the regular glass lights its edge, and the
    /// Dock under Increase Contrast.
    pub specular: GlassSpecular,
    /// The control outline centred on the edge ([`GlassBorder`]): a toolbar item draws one.
    pub border: GlassBorder,
    /// A straight sRGB colour laid over the face in normal blending, after the fill and before the rim; its alpha is
    /// the tint's own times the face opacity its caller asks for ([`GlassParams::with_tint`]). Transparent for none.
    pub tint: [f32; 4],
}

impl GlassParams {
    /// The recipe with `tint` (straight sRGB) laid over its face at the tint's alpha times `face_opacity`.
    pub fn with_tint(self, tint: [f32; 4], face_opacity: f32) -> Self {
        let a = (tint[3] * face_opacity).clamp(0.0, 1.0);
        Self {
            tint: [tint[0], tint[1], tint[2], if a.is_nan() { 0.0 } else { a }],
            ..self
        }
    }
}

/// Light from the top.
const HIGHLIGHT_ANGLE: f32 = std::f32::consts::FRAC_PI_2;

/// The drop shadow of a glass window (an alert, a sheet, a menu): its blur radius and how far below the window it falls
/// in points, and its
/// black's opacity in the light and the dark appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlertShadow {
    pub blur: f32,
    pub offset: f32,
    pub light: f32,
    pub dark: f32,
}

/// The shadow of an alert and of a menu while its window has the keyboard, and the shallower, fainter one while another
/// window has it (recorded).
pub const ALERT_SHADOW_ACTIVE: AlertShadow = AlertShadow {
    blur: 24.0,
    offset: 10.0,
    light: 0.7,
    dark: 0.8,
};
pub const ALERT_SHADOW_INACTIVE: AlertShadow = AlertShadow {
    blur: 16.0,
    offset: 4.0,
    light: 0.55,
    dark: 0.65,
};

impl GlassShadow {
    /// This shadow with an alert's drop shadow shown, for an alert that has the keyboard (`active`) or not, in the dark
    /// appearance or the light one; the ring and the lines along the edge are kept.
    pub fn alert(self, active: bool, dark: bool) -> GlassShadow {
        let s = if active {
            ALERT_SHADOW_ACTIVE
        } else {
            ALERT_SHADOW_INACTIVE
        };
        self.window(s, dark)
    }
}

/// Linear interpolation through `table` (sorted by side), flat outside it.
fn by_side(table: &[(f32, f32)], side: f32) -> f32 {
    let (first, last) = (table[0], table[table.len() - 1]);
    if side <= first.0 {
        return first.1;
    }
    if side >= last.0 {
        return last.1;
    }
    table
        .windows(2)
        .find(|w| side <= w[1].0)
        .map(|w| {
            let t = (side - w[0].0) / (w[1].0 - w[0].0);
            w[0].1 + (w[1].1 - w[0].1) * t
        })
        .unwrap_or(last.1)
}

/// The sheet's shadow while its window has the keyboard, and the fainter one while another window has it: the alert's
/// optics at a lower opacity (recorded).
pub const SHEET_SHADOW_ACTIVE: AlertShadow = AlertShadow {
    blur: 24.0,
    offset: 10.0,
    light: 0.6,
    dark: 0.7,
};
pub const SHEET_SHADOW_INACTIVE: AlertShadow = AlertShadow {
    blur: 16.0,
    offset: 4.0,
    light: 0.45,
    dark: 0.55,
};

impl GlassShadow {
    /// This shadow with a document-modal sheet's drop shadow shown, for a sheet whose window has the keyboard
    /// (`active`) or not; the ring and the lines along the edge are kept.
    pub fn sheet(self, active: bool, dark: bool) -> GlassShadow {
        let s = if active {
            SHEET_SHADOW_ACTIVE
        } else {
            SHEET_SHADOW_INACTIVE
        };
        self.window(s, dark)
    }

    /// This shadow with the window shadow `s` shown in the dark appearance or the light one.
    pub fn window(self, s: AlertShadow, dark: bool) -> GlassShadow {
        GlassShadow {
            offset: [0.0, s.offset],
            amount: 100.0,
            height: s.blur,
            fill: [0.0, 0.0, 0.0, if dark { s.dark } else { s.light }],
            opacity: 1.0,
            ..self
        }
    }
}

mod accessibility;
mod clear;
mod fitted;
mod foreground;
mod recipes;
mod regular;
mod variant;

pub use accessibility::*;
pub use clear::*;
pub use fitted::*;
pub use foreground::*;
pub use recipes::*;
pub use regular::*;
pub use variant::*;
