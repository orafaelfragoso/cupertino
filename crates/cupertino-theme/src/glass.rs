//! Glass material recipes: every value of the background pass (blur, face colour, refraction, bleed, highlight and
//! shadows) and the foreground colour matrix, for a [`GlassMode`] (appearance, style and accessibility options) and the
//! size of the shape it covers.
//!
//! A recipe is plain data. `cupertino-glass` turns it into GPU uniforms and holds the CPU model of each stage;
//! the README of that crate describes what every value does.
//!
//! * [`GlassStyle::Regular`] is the legibility style for controls, bars and panels; [`GlassStyle::Clear`] lets more of
//!   the backdrop through, for controls over media; [`GlassStyle::LargePanel`] is the regular glass of large floating
//!   panels, with a stronger tone compression and a drop shadow of its own.
//! * Size: several values follow the short side of the shape ([`GlassParams::new`]), so a small control bends and
//!   bleeds less than a large panel.
//! * Accessibility selects recipes of its own, it does not turn the glass off: Reduce Transparency trades refraction for
//!   a wide blur and a dense fill, Increase Contrast keeps the optics with darker or lighter colours, both together take
//!   the optics of the first and the colours of the second, and Reduce Motion flattens the refraction and widens the
//!   blur.

/// The scale the backdrop of a glass container is captured at, relative to the output's pixels, unless its recipe
/// says otherwise ([`GlassParams::capture_scale`]).
pub const GLASS_BACKDROP_SCALE: f32 = 0.5;
/// The capture scale of a notification card's glass.
pub const CARD_CAPTURE_SCALE: f32 = 0.25;
/// The capture scale of a track's glass: the output's own.
pub const TRACK_CAPTURE_SCALE: f32 = 1.0;

/// Most blurred copies of the backdrop one container mixes: one per distinct blur of its shapes (each shape's Gaussian
/// follows its size), up to this many; a shape whose blur lies between two of them mixes the two.
pub const BLUR_LEVELS: usize = 4;

/// The Gaussian deviation of a glass blur, in points per point of its radius scaled by the first level's opacity: a
/// regular control (radius 5 at opacity 0.4) blurs with a deviation of 4 points, a large regular panel (opacity 0.8)
/// with 8, clear glass of radius 10 with 20.
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

/// The two glass styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassStyle {
    /// For legibility: controls, bars, menus and panels with text on them.
    #[default]
    Regular,
    /// Lets the backdrop through with little tone change, for controls over photos and video.
    Clear,
    /// Regular glass for large floating panels (the launcher): the same optics with a blur twice as wide and a stronger
    /// tone compression (a lower white point, a lifted black and a little less saturation), so a big surface softens
    /// the scene behind it, and a broad drop shadow of its own.
    LargePanel,
    /// The glass of a floating bar of icons (the dock): a narrow blur, a little more colour, a thin bending edge and a
    /// thin line of light along its top and bottom edges, over a face that lifts black a little and dims white a little.
    Bar,
    /// The glass of a dense floating surface with text on it (a menu, a popover): regular glass with a wider blur over a
    /// face that washes the backdrop out, to a pale pane in the light appearance and a dark smoky one in the dark.
    Dense,
    /// The glass of the tiles of a panel of modules (the control panel): the clear face, which keeps the scene's colour,
    /// a wide blur and a strong bending edge, light in the light appearance and dark in the dark one.
    Module,
    /// The glass of a notification card: the clear face over a wide blur and the blur fill, a bending edge and a faint
    /// ring shadow inside its outline, read from a backdrop captured at a quarter of the output's scale.
    Card,
    /// The glass of a level's track (the control panel's sliders): no blur, the backdrop read at the output's scale, a
    /// slight lift of black, a bending edge that follows the track's height and a ring shadow inside its outline.
    Track,
    /// The glass of a small popover over a floating bar (a tile's name, a tile's menu): the backdrop under a wide soft
    /// blur fill only, a strong bending edge and a thin line of light along its top and bottom edges, the same at every
    /// size.
    Popover,
    /// The glass of a wide floating panel of large icons (the application switcher): a blur of about a point, a strong
    /// bending edge, a face that lifts black and brightens in the light appearance and dims to a smoky pane with less
    /// colour in the dark one, and no shadow.
    Switcher,
    /// The glass of a window's sidebar: regular glass under a wide soft blur fill, over a face that keeps a light pane
    /// in the light appearance and a dark, nearly grey one in the dark, with a faint ring shadow.
    Sidebar,
    /// The module glass of a tile that steps back (a panel of modules behind what is in front of it): twice the blur, a
    /// dimmer white in the light appearance and no lifted black in the dark one.
    SubduedModule,
    /// The glass of a small card of content over the desktop: a faint blur under a wide soft blur fill, over a face
    /// veiled white in the light appearance and black in the dark.
    Snippet,
    /// Clear glass under a soft blur fill, veiled black from a fifth at its top to fully at its bottom, so it reads as
    /// a dark gradient that keeps the backdrop's colour at the top.
    Veiled,
    /// The glass of a window's toolbar controls: regular glass with a face lifted a little further from the content
    /// under it, a thin line of light along its top edge (fainter along its bottom) over a soft glow, and no rim or edge
    /// line of its own, as the toolkit draws the control's outline.
    Toolbar,
    /// The glass of a menu: a wide blur under a face that tones the backdrop into a narrow band, a pale wash of it in the
    /// light appearance and in the dark one a mid-dark pane that lifts a dark backdrop and holds a bright one down, with
    /// a thin line of light along its top and bottom edges and no bending edge.
    Menu,
}

/// Everything besides the shape's size that selects a recipe.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlassMode {
    pub dark: bool,
    pub style: GlassStyle,
    pub reduce_transparency: bool,
    pub increase_contrast: bool,
    pub reduce_motion: bool,
    /// How much denser the person asked every material to be, in percent (0 leaves the recipe as it is).
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
    /// The key and fill light added along the edge ([`GlassSpecular`]); none in the recipes of today.
    pub specular: GlassSpecular,
    /// The control outline centred on the edge ([`GlassBorder`]); none in the recipes of today.
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

// ---- size rules -------------------------------------------------------------------------------------------------

/// Largest refraction strength of any shape.
const REFRACTION_AMOUNT_MAX: f32 = 60.0;
/// Refraction strength per point of short side, regular style.
const REFRACTION_PER_POINT_REGULAR: f32 = 0.5;
/// Refraction strength per point of short side, clear style.
const REFRACTION_PER_POINT_CLEAR: f32 = 0.65;
/// Widest refraction band in points.
const REFRACTION_HEIGHT_MAX: f32 = 20.0;
/// Refraction band width per point of short side, regular style.
const REFRACTION_HEIGHT_PER_POINT_REGULAR: f32 = 0.25;
/// Refraction band width per point of short side, clear style.
const REFRACTION_HEIGHT_PER_POINT_CLEAR: f32 = 0.36;
/// Bleed strength and width per point of short side (regular style).
const BLEED_PER_POINT: f32 = 0.35;
/// The widest blur of the backdrop the bleed reads, in points.
const BLEED_BLUR_MAX: f32 = 160.0;
/// Short side past which the bleed stops growing.
const BLEED_SIDE_MAX: f32 = 240.0;
/// Opacity of the first blur level of a regular shape by short side `(side, opacity)`, which scales its blur radius;
/// linear between, flat outside: a small control blurs with 0.4 of the radius, a panel 160 points high or more with 0.8.
const NARROW_LEVEL_OPACITY_BY_SIDE: [(f32, f32); 2] = [(64.0, 0.4), (160.0, 0.8)];
/// Largest face luminance of dark regular glass by short side `(side, max luma)`; linear between, flat outside.
const DARK_MAX_LUMA_BY_SIDE: [(f32, f32); 5] = [
    (60.0, 0.6),
    (80.0, 0.5),
    (100.0, 0.375),
    (120.0, 0.35),
    (240.0, 0.358),
];
/// Blur radius of the smallest clear shapes, in points.
const CLEAR_BLUR_MIN: f32 = 5.0;
/// Blur radius of the largest clear shapes.
const CLEAR_BLUR_MAX: f32 = 10.0;
/// Short side where the clear blur starts to grow.
const CLEAR_BLUR_SIDE_MIN: f32 = 48.0;
/// Blur gained per point of short side.
const CLEAR_BLUR_PER_POINT: f32 = 5.0 / 72.0;
/// Clear drop-shadow strength per point of short side, and the side it stops growing at.
const CLEAR_SHADOW_AMOUNT_PER_POINT: f32 = 0.625;
const CLEAR_SHADOW_AMOUNT_SIDE_MAX: f32 = 120.0;
/// Clear drop-shadow blur per point of short side, and the side it stops growing at.
const CLEAR_SHADOW_HEIGHT_PER_POINT: f32 = 0.4;
const CLEAR_SHADOW_HEIGHT_SIDE_MAX: f32 = 240.0;

/// The share of what is still see-through that the densest setting fills with the face's own colour.
const DENSITY_FILL_SHARE: f32 = 0.3;
/// The colour of a fill that has none of its own (a clear face) once it is made denser: dark and light.
const DENSITY_NEUTRAL: [f32; 3] = [0.12, 0.12, 0.12];
const DENSITY_NEUTRAL_LIGHT: [f32; 3] = [0.95, 0.95, 0.95];

/// `fill` made denser by `amount` (0 to 1): its opacity grows towards [`DENSITY_FILL_SHARE`] of what was see-through.
fn denser_fill(fill: [f32; 4], dark: bool, amount: f32) -> [f32; 4] {
    let colour = if fill[3] < 0.02 {
        if dark {
            DENSITY_NEUTRAL
        } else {
            DENSITY_NEUTRAL_LIGHT
        }
    } else {
        [fill[0], fill[1], fill[2]]
    };
    let alpha = fill[3] + (1.0 - fill[3]) * DENSITY_FILL_SHARE * amount;
    [colour[0], colour[1], colour[2], alpha]
}

// ---- accessibility ----------------------------------------------------------------------------------------------

/// Blur radius of regular glass with Reduce Transparency, in points.
const REDUCED_TRANSPARENCY_BLUR: f32 = 40.625;
/// Blur radius of clear glass with Reduce Transparency.
const REDUCED_TRANSPARENCY_BLUR_CLEAR: f32 = 200.0;
/// Level opacities with Reduce Transparency: the two narrow levels win.
const REDUCED_TRANSPARENCY_LEVELS: [f32; BLUR_LEVELS] = [1.0, 1.0, 0.123_076_92, 0.123_076_92];
/// Smallest blur radius with Reduce Motion.
const REDUCED_MOTION_BLUR: f32 = 8.0;
/// Level opacities with Reduce Motion.
const REDUCED_MOTION_LEVELS: [f32; BLUR_LEVELS] = [1.0, 1.0, 0.625, 0.625];

// ---- colour matrices --------------------------------------------------------------------------------------------

/// Key light colour of regular glass: a little more saturated and lifted than the face.
const HIGHLIGHT_MATRIX_REGULAR: ColorMatrix = [
    1.1202, -0.1894, -0.019, 0.0, 0.1471, //
    -0.0563, 0.9871, -0.0191, 0.0, 0.1471, //
    -0.0563, -0.1893, 1.1574, 0.0, 0.1471, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// Key light colour of clear glass: brighter, keeping the hue.
const HIGHLIGHT_MATRIX_CLEAR: ColorMatrix = [
    1.3192, -0.0478, -0.0047, 0.0, 0.2, //
    -0.0142, 1.2856, -0.0048, 0.0, 0.2, //
    -0.0142, -0.0476, 1.3285, 0.0, 0.2, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// Foreground on dark glass: the face under the glyph lifted almost to white, so a label keeps a trace of the colour
/// behind it.
const FOREGROUND_MATRIX_DARK: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, 0.95, //
    0.0, 1.0, 0.0, 0.0, 0.95, //
    0.0, 0.0, 1.0, 0.0, 0.95, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// Foreground on light glass: the face under the glyph pushed down to black.
const FOREGROUND_MATRIX_LIGHT: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, -1.0, //
    0.0, 1.0, 0.0, 0.0, -1.0, //
    0.0, 0.0, 1.0, 0.0, -1.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// Light from the top.
const HIGHLIGHT_ANGLE: f32 = std::f32::consts::FRAC_PI_2;

fn regular_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: 1.3,
            white: 1.125,
            black: 0.08,
            clamp: 1.308_310_9,
            max_luma: 0.375,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: REGULAR_SATURATION_LIGHT,
            white: REGULAR_WHITE_LIGHT,
            black: REGULAR_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, REGULAR_FILL_LIGHT],
        }
    }
}

/// The light regular face: a little more colour, black lifted by 0.4 of what is left and white dimmed a little, under a
/// white veil, so a dark backdrop comes through as a light grey of it (black comes out at 130 of 255, white at 240).
const REGULAR_SATURATION_LIGHT: f32 = 1.2;
const REGULAR_WHITE_LIGHT: f32 = 0.96;
const REGULAR_BLACK_LIGHT: f32 = 0.40;
const REGULAR_FILL_LIGHT: f32 = 0.184;
/// The edge line of regular glass in the light and the dark appearance ([`GlassShadow::edge_line`]).
const REGULAR_EDGE_LINE_LIGHT: f32 = 0.2;
const REGULAR_EDGE_LINE_DARK: f32 = 0.22;

fn clear_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: 1.0,
            white: 0.8,
            black: 0.05,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.05],
        }
    } else {
        GlassFace {
            saturation: 1.0,
            white: 0.95,
            black: 0.2,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.1],
        }
    }
}

/// The face of bar glass: a little more colour, black lifted and white dimmed a little. Light: black 0.15, white 0.9.
/// Dark: black 0.05, white 0.92, the luma compressed to 0.75 and a faint black veil.
fn bar_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: BAR_SATURATION,
            white: 0.92,
            black: 0.05,
            clamp: 1.0,
            max_luma: 0.75,
            fill: [0.0, 0.0, 0.0, BAR_VEIL_DARK],
        }
    } else {
        GlassFace {
            saturation: BAR_SATURATION,
            white: 0.9,
            black: 0.15,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, 0.0],
        }
    }
}

/// The face of popover glass. Light: the regular light face with white a little past 1. Dark: black lifted by an
/// eighth, white past 1 and the luma compressed to 0.6, with more colour.
fn popover_face(dark: bool) -> GlassFace {
    let regular = regular_face(dark);
    if dark {
        GlassFace {
            black: POPOVER_BLACK_DARK,
            max_luma: POPOVER_MAX_LUMA_DARK,
            ..regular
        }
    } else {
        GlassFace {
            white: POPOVER_WHITE_LIGHT,
            ..regular
        }
    }
}

/// The face of switcher glass. Light: black lifted by a quarter, white a little past 1, more colour and the luma
/// compressed to 0.85, clamped a little past 1. Dark: black 0.05, white dimmed to 0.3 and less colour.
fn switcher_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: SWITCHER_SATURATION_DARK,
            white: SWITCHER_WHITE_DARK,
            black: SWITCHER_BLACK_DARK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: SWITCHER_SATURATION_LIGHT,
            white: SWITCHER_WHITE_LIGHT,
            black: SWITCHER_BLACK_LIGHT,
            clamp: SWITCHER_CLAMP_LIGHT,
            max_luma: SWITCHER_MAX_LUMA_LIGHT,
            fill: [0.0; 4],
        }
    }
}

/// The switcher's blur radius in points, every level at full opacity, with no blur fill.
const SWITCHER_BLUR: f32 = 0.96;
/// The switcher's face values (see [`switcher_face`]).
const SWITCHER_SATURATION_LIGHT: f32 = 1.2;
const SWITCHER_BLACK_LIGHT: f32 = 0.25;
const SWITCHER_WHITE_LIGHT: f32 = 1.06;
const SWITCHER_MAX_LUMA_LIGHT: f32 = 0.85;
const SWITCHER_CLAMP_LIGHT: f32 = 1.142;
const SWITCHER_SATURATION_DARK: f32 = 0.8;
const SWITCHER_BLACK_DARK: f32 = 0.05;
const SWITCHER_WHITE_DARK: f32 = 0.3;
/// The switcher's bending edge: its strength and the depth of its band, in points, the same at every size.
const SWITCHER_REFRACTION: f32 = -41.6;
const SWITCHER_REFRACTION_HEIGHT: f32 = 9.6;
/// The switcher's rim: its strength and the colour shift per unit of it.
const SWITCHER_RIM: f32 = 0.4;
const SWITCHER_RIM_BIAS: f32 = -0.5;
/// The opacity of the dark line just outside the switcher's outline in the dark appearance (the light one keeps
/// regular glass's).
const SWITCHER_EDGE_LINE_DARK: f32 = 0.32;

/// Sidebar glass: the blur at full opacity of every level, the blur fill's radius and its lighten (light) or darken
/// (dark) and normal opacities, the bending edge and band in points, the rim and its colour shift, and the ring shadow.
const SIDEBAR_BLUR: f32 = 5.0;
const SIDEBAR_FILL_RADIUS: f32 = 8.0;
const SIDEBAR_FILL_BLEND: f32 = 0.882;
const SIDEBAR_FILL_NORMAL: f32 = 0.48;
const SIDEBAR_REFRACTION: f32 = -24.0;
const SIDEBAR_REFRACTION_HEIGHT: f32 = 12.0;
const SIDEBAR_RIM: f32 = 0.5;
const SIDEBAR_RIM_BIAS: f32 = -0.188;
const SIDEBAR_RING_OPACITY: f32 = 0.06;

/// The face of sidebar glass: a dense pane that lets only a trace of the backdrop through. Dark: a grey `x` comes out at
/// `SIDEBAR_BLACK_DARK + (SIDEBAR_WHITE_DARK − SIDEBAR_BLACK_DARK) · x`, about `0.034 · x + 34` of 255 (black near 34,
/// white near 42). Light: the levels under a half veil of a warm light grey, about `0.02 · x + 229` (black near 229,
/// white near 234). The backdrop's colour comes through at [`SIDEBAR_SATURATION`] of its departure from grey, so the pane
/// only takes a faint tint of what lies behind it.
fn sidebar_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: SIDEBAR_SATURATION,
            white: SIDEBAR_WHITE_DARK,
            black: SIDEBAR_BLACK_DARK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: SIDEBAR_SATURATION,
            white: SIDEBAR_WHITE_LIGHT,
            black: SIDEBAR_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: 1.0,
            fill: SIDEBAR_VEIL_LIGHT,
        }
    }
}

/// How much of the backdrop's departure from grey the sidebar keeps, in both appearances.
pub const SIDEBAR_SATURATION: f32 = 0.1;
/// The dark sidebar's black and white levels.
pub const SIDEBAR_BLACK_DARK: f32 = 0.1325;
pub const SIDEBAR_WHITE_DARK: f32 = 0.1665;
/// The light sidebar's black and white levels and the warm light grey veil over them.
pub const SIDEBAR_BLACK_LIGHT: f32 = 0.9;
pub const SIDEBAR_WHITE_LIGHT: f32 = 0.94;
pub const SIDEBAR_VEIL_LIGHT: [f32; 4] = [0.9094, 0.8969, 0.8827, 0.5];

/// Subdued module glass: how much wider its blur is than the module's of the same size, and its face's white (light)
/// and black (dark).
const SUBDUED_MODULE_BLUR_SCALE: f32 = 2.0;
const SUBDUED_MODULE_WHITE_LIGHT: f32 = 0.88;
const SUBDUED_MODULE_BLACK_DARK: f32 = 0.0;

/// Snippet glass: the blur radius and the first level's opacity, the blur fill, the bending edge, the rim and the ring.
const SNIPPET_BLUR: f32 = 5.0;
const SNIPPET_LEVEL_OPACITY: f32 = 0.2666;
const SNIPPET_FILL_RADIUS: f32 = 8.0;
const SNIPPET_FILL_BLEND: f32 = 0.882;
const SNIPPET_FILL_NORMAL: f32 = 0.46;
const SNIPPET_REFRACTION: f32 = -24.0;
const SNIPPET_REFRACTION_HEIGHT: f32 = 12.0;
const SNIPPET_RIM: f32 = 0.4;
const SNIPPET_RIM_BIAS: f32 = -0.25;
const SNIPPET_RING_OPACITY: f32 = 0.06;
/// The edge line of light snippet glass; the dark one has none.
const SNIPPET_EDGE_LINE_LIGHT: f32 = 0.1;

/// The face of snippet glass. Light: the regular light face with white past 1, a luma cap of 0.94 and a clamp of
/// 1.07, under a white veil of 0.5. Dark: more colour, black lifted by an eighth, white past 1, the luma compressed to
/// 0.6 and a clamp of 1.31, under a black veil of 0.4.
fn snippet_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: 1.3,
            white: 1.12,
            black: 0.125,
            clamp: 1.31,
            max_luma: 0.6,
            fill: [0.0, 0.0, 0.0, 0.4],
        }
    } else {
        GlassFace {
            saturation: 1.2,
            white: 1.03,
            black: 0.4,
            clamp: 1.07,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, 0.5],
        }
    }
}

/// Veiled glass: the blur, the blur fill (lighten in the light appearance only, darken and normal in both), the
/// bending edge, the rim, and the black veil's opacity at the top and the bottom.
const VEILED_BLUR: f32 = 5.0;
const VEILED_FILL_RADIUS: f32 = 8.0;
const VEILED_FILL_LIGHTEN: f32 = 0.882;
const VEILED_FILL_DARKEN: f32 = 0.75;
const VEILED_FILL_NORMAL: f32 = 0.5;
const VEILED_REFRACTION: f32 = -34.56;
const VEILED_REFRACTION_HEIGHT: f32 = 19.2;
const VEILED_RIM: f32 = 0.4;
const VEILED_RIM_BIAS: f32 = -0.25;
const VEILED_VEIL: [f32; 2] = [0.2, 1.0];

/// The side the popover's regular glass is worked out at before its own values replace what follows the size.
const POPOVER_SIDE: f32 = 26.0;
/// The popover's blur: its radius, and the opacity of each level, the first 0, so only the blur fill softens it.
const POPOVER_BLUR: f32 = 10.0;
const POPOVER_LEVELS: [f32; BLUR_LEVELS] = [0.0, 0.0, 0.5, 1.0];
/// The popover's blur fill: its radius, its opacity in darken (dark) or lighten (light) mode, and in normal mode.
const POPOVER_FILL_RADIUS: f32 = 8.0;
const POPOVER_FILL_OVER: f32 = 0.882;
const POPOVER_FILL_NORMAL: f32 = 0.46;
/// The popover's refraction: strength and band width in points.
const POPOVER_REFRACTION: f32 = -24.0;
const POPOVER_REFRACTION_HEIGHT: f32 = 5.0;
/// The popover's bleed band (shown at no opacity at rest).
const POPOVER_BLEED: f32 = 16.8;
/// The faint ring shadow inside the popover's outline.
const POPOVER_RING_OPACITY: f32 = 0.06;
/// The popover's edge line in the light appearance; the dark popover has none.
const POPOVER_EDGE_LINE_LIGHT: f32 = 0.175;
/// The dark popover face's black lift and luma compression, and the light face's white point.
const POPOVER_BLACK_DARK: f32 = 0.125;
const POPOVER_MAX_LUMA_DARK: f32 = 0.6;
const POPOVER_WHITE_LIGHT: f32 = 1.03;

/// The saturation of the bar's face in both appearances.
const BAR_SATURATION: f32 = 1.25;
/// The black veil over the dark bar's face.
const BAR_VEIL_DARK: f32 = 0.02;
/// The bar's blur radius in points, at full opacity of the first level.
const BAR_BLUR: f32 = 1.0;
/// The bar's refraction: strength and band width in points, the same at every size.
const BAR_REFRACTION: f32 = -8.64;
const BAR_REFRACTION_HEIGHT: f32 = 3.84;
/// How strongly a soft band lights the far end of its axis, as a share of the near end, unless a surface says
/// otherwise: on clear glass and on regular glass (and the styles built on it).
const GLOW_FILL: f32 = 0.75;
const GLOW_FILL_REGULAR: f32 = 0.8125;
/// The bar's rim: strength and colour shift per unit of strength.
const BAR_RIM: f32 = 0.4;
const BAR_RIM_BIAS: f32 = -0.25;
/// The bar's line of light along its top and bottom edges: strength, half-width and middle (points from the edge, inside
/// for a negative value), the arcs' angular width, and the bottom's strength as a share of the top's.
const BAR_GLOW: f32 = 0.5;
const BAR_GLOW_HEIGHT: f32 = 0.6;
const BAR_GLOW_OFFSET: f32 = -0.25;
const BAR_GLOW_SPREAD: f32 = 80.0 * std::f32::consts::PI / 180.0;
const BAR_GLOW_FILL: f32 = 1.0;
/// The faint glow under the bar's line of light: eight times as wide, on arcs 0.65 as wide, at 0.15 of its strength.
const BAR_GLOW_DIFFUSE: GlowDiffuse = GlowDiffuse {
    height_scale: 8.0,
    spread_scale: 0.65,
    amount_scale: 0.15,
};
/// The bar's drop shadow (shown at no opacity at rest): strength in percent and blur in points.
const BAR_SHADOW_AMOUNT: f32 = 30.0;
const BAR_SHADOW_HEIGHT: f32 = 19.2;
/// The colour of the bar's soft light from the face under it: a little more saturated and lifted than the face, in the
/// light and the dark appearance.
const BAR_GLOW_MATRIX_LIGHT: ColorMatrix = [
    1.2625, -0.2385, -0.024, 0.0, 0.2, //
    -0.0708, 1.0949, -0.0241, 0.0, 0.2, //
    -0.0709, -0.2383, 1.3093, 0.0, 0.2, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
const BAR_GLOW_MATRIX_DARK: ColorMatrix = [
    1.0937, -0.3577, -0.036, 0.0, 0.25, //
    -0.1063, 0.8424, -0.0361, 0.0, 0.25, //
    -0.1064, -0.3575, 1.1639, 0.0, 0.25, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The toolbar face: the dark one lifts black by 0.11 rather than 0.08, so the window background under a control (31 of
/// 255) comes out at 58; the light one keeps white at 0.99, so a white background comes out at 245.
const TOOLBAR_BLACK_DARK: f32 = 0.11;
const TOOLBAR_WHITE_LIGHT: f32 = 0.99;
/// The toolbar's line of light: strength in the dark and the light appearance (the light face is near white, so the light
/// added to it is brighter and a weaker line lifts it as far), half-width and middle (points from the edge, inside for a
/// negative value), the arcs' angular width (lit to the corners' diagonals, dark across the middle of the sides) and
/// the bottom's strength as a share of the top's.
const TOOLBAR_GLOW_DARK: f32 = 0.7;
const TOOLBAR_GLOW_LIGHT: f32 = 0.25;
const TOOLBAR_GLOW_HEIGHT: f32 = 0.45;
const TOOLBAR_GLOW_OFFSET: f32 = -0.25;
const TOOLBAR_GLOW_SPREAD: f32 = 140.0 * std::f32::consts::PI / 180.0;
const TOOLBAR_GLOW_FILL: f32 = 0.5;
/// The soft glow under the toolbar's line of light: eight times as wide, on arcs as wide, at 0.12 of its strength.
const TOOLBAR_GLOW_DIFFUSE: GlowDiffuse = GlowDiffuse {
    height_scale: 8.0,
    spread_scale: 1.0,
    amount_scale: 0.12,
};

/// The face of dense glass, before its size sets the dark face's luma compression. Dark: a little more colour, black
/// lifted by 0.15 and white dimmed to 0.3, so any backdrop comes through as a dark smoky grey of it. Light: black lifted
/// by 0.8 and white dimmed a little under a faint light grey veil, so any backdrop comes through as a pale wash of it.
fn dense_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: DENSE_SATURATION_DARK,
            white: DENSE_WHITE_DARK,
            black: DENSE_BLACK_DARK,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            black: DENSE_BLACK_LIGHT,
            clamp: 1.0,
            fill: DENSE_FILL_LIGHT,
            ..regular_face(false)
        }
    }
}

/// Saturation and white and black levels of the dark dense face; its luma compression follows the size as regular
/// glass's does.
const DENSE_SATURATION_DARK: f32 = 1.3;
const DENSE_WHITE_DARK: f32 = 0.3;
const DENSE_BLACK_DARK: f32 = 0.15;
/// The light dense face's black level and the straight sRGB veil over it.
const DENSE_BLACK_LIGHT: f32 = 0.8;
const DENSE_FILL_LIGHT: [f32; 4] = [0.96, 0.96, 0.96, 0.04];
/// The dense glass's blur radius by short side `(side, radius)`, at full opacity of the first level; linear between,
/// flat outside.
const DENSE_BLUR_BY_SIDE: [(f32, f32); 2] = [(60.0, 6.0), (120.0, 12.0)];
/// The dense glass's refraction strength and band width per point of short side, at most the regular glass's.
const DENSE_REFRACTION_PER_POINT: f32 = 0.4;
const DENSE_REFRACTION_HEIGHT_PER_POINT: f32 = 0.2;
/// The faint ring shadow inside the dense glass's outline.
const DENSE_RING_OPACITY: f32 = 0.06;
/// The dense glass's edge line, light and dark appearance.
const DENSE_EDGE_LINE_LIGHT: f32 = 0.31;
const DENSE_EDGE_LINE_DARK: f32 = 0.54;
/// The scale the dense glass's backdrop is captured at: a quarter of the output's, an eighth once its blur is as wide as
/// it grows.
const DENSE_CAPTURE_SCALE: f32 = 0.25;
const DENSE_CAPTURE_SCALE_WIDE: f32 = 0.125;

/// The face of menu glass. Light: a pale wash of the backdrop, about `0.41 · c + 144` of 255 per channel over the
/// backdrop's range (white comes out near 248, black near 113). Dark: the luma compression bends the tone curve over, so a
/// dark backdrop is lifted (black near 22, a grey of 64 near 76) and a bright one held near 112 to 115, keeping its
/// colour (white comes out near 102).
fn menu_face(dark: bool) -> GlassFace {
    if dark {
        GlassFace {
            saturation: MENU_SATURATION_DARK,
            white: MENU_WHITE_DARK,
            black: MENU_BLACK_DARK,
            clamp: 1.0,
            max_luma: MENU_MAX_LUMA_DARK,
            fill: [0.0; 4],
        }
    } else {
        GlassFace {
            saturation: MENU_SATURATION_LIGHT,
            white: MENU_WHITE_LIGHT,
            black: MENU_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: MENU_MAX_LUMA_LIGHT,
            fill: [0.0; 4],
        }
    }
}

/// The dark menu face: saturation, white and black levels and luma compression.
pub const MENU_SATURATION_DARK: f32 = 1.193;
pub const MENU_WHITE_DARK: f32 = 1.0966;
pub const MENU_BLACK_DARK: f32 = 0.0877;
pub const MENU_MAX_LUMA_DARK: f32 = 0.3096;
/// The light menu face: saturation, white and black levels and luma compression.
pub const MENU_SATURATION_LIGHT: f32 = 1.2271;
pub const MENU_WHITE_LIGHT: f32 = 1.2825;
pub const MENU_BLACK_LIGHT: f32 = 0.4413;
pub const MENU_MAX_LUMA_LIGHT: f32 = 0.6321;
/// The menu's blur radius at full opacity of the first level: a Gaussian of 13.5 points.
pub const MENU_BLUR: f32 = 6.75;
/// The scale the menu's backdrop is captured at.
const MENU_CAPTURE_SCALE: f32 = 0.25;
/// The line of light along the menu's top and bottom edges, in the dark and the light appearance: its strength (white
/// added at its middle), its half-width and where its middle lies inside the edge, in points. Dark: one device pixel
/// at 2× about 48 levels over the face; light: a point about 100 levels over the face, white over a pale one.
const MENU_LINE_DARK: (f32, f32, f32) = (0.19, 0.3, -0.25);
const MENU_LINE_LIGHT: (f32, f32, f32) = (0.5, 0.5, -0.5);
/// The angular width of the arcs the menu's line lights, in radians: the whole top and bottom, fading round the corners.
const MENU_LINE_SPREAD: f32 = std::f32::consts::FRAC_PI_2;
/// The colour of the menu's line of light: white, whatever the face under it.
const MENU_LINE_MATRIX: ColorMatrix = [
    0.0, 0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// Module glass: the clear glass of its size with a strong bending edge and a darker rim. The strength is the same at
/// every size; the band is [`MODULE_REFRACTION_HEIGHT_PER_POINT`] of the short side, at most
/// [`MODULE_REFRACTION_HEIGHT_MAX`] points.
const MODULE_REFRACTION: f32 = -80.0;
const MODULE_REFRACTION_HEIGHT_PER_POINT: f32 = 0.18;
const MODULE_REFRACTION_HEIGHT_MAX: f32 = 25.0;
/// The module's blur radius by short side `(side, radius)`; linear between, flat outside.
const MODULE_BLUR_BY_SIDE: [(f32, f32); 2] = [(60.0, 5.0), (140.0, 10.0)];
const MODULE_RIM_BIAS: f32 = -0.3;
/// The module's face, the same in both appearances: a little more colour, black lifted to about 27 of 255 and white held
/// near 178, so every channel comes out at about `0.59 · c + 27` and a sunlit backdrop as a mid tone of its own colour.
pub const MODULE_SATURATION: f32 = 1.045;
pub const MODULE_WHITE: f32 = 0.697;
pub const MODULE_BLACK: f32 = 0.107;
/// The module's line of light just inside its top and bottom edges: its strength (the share of its colour added at its
/// middle), its half-width and where its middle lies inside the edge, in points, and the bottom's strength as a share
/// of the top's: a bright line about a point wide along the top and as bright along the bottom, which takes a
/// mid-toned face to near white.
pub const MODULE_LINE: (f32, f32, f32) = (0.55, 0.55, -0.45);
pub const MODULE_LINE_BOTTOM: f32 = 1.0;
/// The exponent on the module line's arc falloff: under 1, so the line keeps most of its light round the corners until
/// the side's dark line takes over.
const MODULE_LINE_CURVATURE: f32 = 0.5;
/// The colour of the module's line of light from the face under it: the face plus nearly half white, so a mid tone of
/// a warm backdrop turns a pale tint of it and a light one white.
const MODULE_LINE_MATRIX: ColorMatrix = [
    1.0, 0.0, 0.0, 0.0, 0.47, //
    0.0, 1.0, 0.0, 0.0, 0.47, //
    0.0, 0.0, 1.0, 0.0, 0.47, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];
/// The dark line just outside the module's outline, which marks its sides: its strength, the same over any backdrop,
/// and the angular width of the arcs facing left and right it keeps to (none along the top and the bottom, which carry
/// the line of light).
const MODULE_EDGE_LINE: f32 = 0.64;
const MODULE_EDGE_LINE_SIDES: f32 = std::f32::consts::FRAC_PI_2;

/// Card glass: the blur radius at full opacity of the first level, the blur fill's radius and opacities (lighten in the
/// light appearance, darken in the dark one, and normal), the refraction, the rim and the ring shadow inside the
/// outline, which shows at rest.
const CARD_BLUR: f32 = 10.0;
const CARD_FILL_RADIUS: f32 = 8.0;
const CARD_FILL_BLEND: f32 = 0.8821;
const CARD_FILL_NORMAL: f32 = 0.4602;
const CARD_REFRACTION: f32 = -60.0;
const CARD_REFRACTION_HEIGHT: f32 = 20.0;
const CARD_RIM: f32 = 0.4;
const CARD_RIM_BIAS: f32 = -0.25;
/// The angular width of the card's light, light and dark appearance, in radians (96° and 75°).
const CARD_SPREAD_LIGHT: f32 = 1.676;
const CARD_SPREAD_DARK: f32 = 5.0 * std::f32::consts::PI / 12.0;
const CARD_RING_OPACITY: f32 = 0.06;
/// The card's edge line in the light appearance, all round.
const CARD_EDGE_LINE_LIGHT: f32 = 0.19;
/// The dark card's edges, as the module's: a dark line just outside its sides only (its strength over any backdrop,
/// on arcs as wide as the module's), and a line of light just inside its top and bottom (its strength, half-width and
/// middle in points, in the module line's colour), about a point wide and some 50 levels over a dark face.
const CARD_EDGE_LINE_DARK: f32 = 0.62;
const CARD_LINE_DARK: (f32, f32, f32) = (0.28, 0.45, -0.4);
/// The dark card's face: a mid-dark pane that holds a bright backdrop near 115 of 255 with a little of its colour, about
/// `0.42 · c + 26` per channel over the bright half of the range, and lets a dark one through darker still (a grey of
/// 128 near 86, black at black). A little less colour than the backdrop's, white kept, no black lift and a strong luma
/// compression, which holds a bright neutral below a bright warm colour; no veil.
pub const CARD_SATURATION_DARK: f32 = 0.794;
pub const CARD_WHITE_DARK: f32 = 0.884;
pub const CARD_BLACK_DARK: f32 = 0.0;
pub const CARD_MAX_LUMA_DARK: f32 = 0.518;

/// Track glass: the face lifts black by a tenth and leaves white; the refraction's strength and band follow the
/// track's height (its short side); the rim; the ring shadow inside the outline, darker in the dark appearance; and a
/// faint bluish shadow below it.
const TRACK_BLACK: f32 = 0.1;
const TRACK_REFRACTION_PER_POINT: f32 = 0.3;
const TRACK_REFRACTION_HEIGHT_PER_POINT: f32 = 0.18;
const TRACK_RIM: f32 = 0.5;
const TRACK_RIM_BIAS: f32 = -0.225;
const TRACK_RING_BLUR: f32 = 3.0;
const TRACK_RING_LIGHT: f32 = 0.15;
const TRACK_RING_DARK: f32 = 0.40;
const TRACK_SHADOW_OFFSET: f32 = 7.0;
const TRACK_SHADOW_AMOUNT: f32 = 6.0;
const TRACK_SHADOW_BLUR: f32 = 8.0;
const TRACK_SHADOW_OPACITY: f32 = 0.0018;
const TRACK_SHADOW_COLOR: [f32; 4] = [0.337, 0.445, 0.562, 1.0];
/// The track's edge line, light and dark appearance.
const TRACK_EDGE_LINE_LIGHT: f32 = 0.22;
const TRACK_EDGE_LINE_DARK: f32 = 0.24;

/// Face colours with Reduce Transparency.
fn reduced_transparency_face(dark: bool, style: GlassStyle) -> GlassFace {
    match (style, dark) {
        (GlassStyle::Clear, true) => GlassFace {
            saturation: 0.7,
            white: 0.0,
            black: 0.075,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [0.0, 0.0, 0.0, 0.25],
        },
        (_, true) => GlassFace {
            saturation: 0.7,
            white: 0.2,
            black: 0.2,
            clamp: 1.0,
            max_luma: 0.375,
            fill: [0.0, 0.0, 0.0, 0.6],
        },
        // light clear glass takes the light regular colours: the denser of the two choices
        (_, false) => GlassFace {
            saturation: 0.9,
            white: 1.03,
            black: 0.9,
            clamp: 1.069_610_1,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, 0.8],
        },
    }
}

/// Face colours with Increase Contrast; clear glass keeps its own, and so does the sidebar, whose dense face already
/// keeps the backdrop from showing through.
fn increased_contrast_face(dark: bool, style: GlassStyle) -> Option<GlassFace> {
    match (style, dark) {
        (GlassStyle::Clear | GlassStyle::Sidebar, _) => None,
        (_, true) => Some(GlassFace {
            saturation: 0.8,
            white: 0.3,
            black: 0.1,
            clamp: 1.0,
            max_luma: 0.375,
            fill: [0.12, 0.12, 0.12, 0.4],
        }),
        (_, false) => Some(GlassFace {
            saturation: 0.8,
            white: 0.96,
            black: 0.5,
            clamp: 1.0,
            max_luma: 0.94,
            fill: [1.0, 1.0, 1.0, 0.45],
        }),
    }
}

/// Face of the large panel. Dark: white behind it comes out a mid grey (about 125 of 255) and black a dark grey (about
/// 53), while strong colours keep much of their brightness, as their luma is compressed less; the saturation is a
/// little under the regular glass's. Light: a bright sheet that keeps the scene's colour: every channel comes out at
/// about `0.61 · c + 124` of 255 after a moderate saturation, so black comes through as a light grey near 124, a dark warm
/// scene as a light tint of its own hue, and anything from a mid tone up is near white.
fn large_panel_face(dark: bool) -> GlassFace {
    let regular = regular_face(dark);
    if dark {
        GlassFace {
            saturation: LARGE_PANEL_SATURATION_DARK,
            white: LARGE_PANEL_WHITE_DARK,
            black: LARGE_PANEL_BLACK_DARK,
            max_luma: LARGE_PANEL_MAX_LUMA_DARK,
            ..regular
        }
    } else {
        GlassFace {
            saturation: LARGE_PANEL_SATURATION_LIGHT,
            white: LARGE_PANEL_WHITE_LIGHT,
            black: LARGE_PANEL_BLACK_LIGHT,
            clamp: 1.0,
            max_luma: 1.0,
            fill: [1.0, 1.0, 1.0, LARGE_PANEL_FILL_LIGHT],
        }
    }
}

/// The light large panel's face: saturation, white and black levels (no luma compression) and the white veil over it.
/// A grey `x` comes out at `(1 − fill) · (b + (w − b) · x) + fill`: 0.61 per unit of the scene above 0.486.
pub const LARGE_PANEL_SATURATION_LIGHT: f32 = 1.4;
pub const LARGE_PANEL_WHITE_LIGHT: f32 = 1.118;
pub const LARGE_PANEL_BLACK_LIGHT: f32 = 0.37;
pub const LARGE_PANEL_FILL_LIGHT: f32 = 0.184;
/// The dark large panel's saturation with Reduce Transparency: the near-black sheet keeps a tenth or so of the
/// backdrop's colour (its departure from grey), so a strong colour behind shows only as a faint tint.
pub const LARGE_PANEL_REDUCED_SATURATION_DARK: f32 = 0.3;
/// The large panel's refraction band: at most this many points inside its edge bend, so a launcher-sized sheet shows
/// no wide lens along its sides.
pub const LARGE_PANEL_REFRACTION_HEIGHT_MAX: f32 = 6.0;
/// The light large panel's bright inner line: a narrow band of light just inside the dark edge, strongest along the
/// top and still showing along the bottom: its strength, half-width and depth in points.
pub const LARGE_PANEL_INNER_LINE_LIGHT: f32 = 0.6;
pub const LARGE_PANEL_INNER_LINE_WIDTH: f32 = 0.35;
pub const LARGE_PANEL_INNER_LINE_DEPTH: f32 = -0.6;
/// The dark large panel's bevel: a bright rim about a point wide just inside its dark edge, which mixes the face this
/// share of the way to white at its middle, its half-width and depth in points. With the arcs below it lights the top
/// and the bottom fully and the sides at about 0.7, so the panel reads as raised rather than outlined.
pub const LARGE_PANEL_RIM_DARK: (f32, f32, f32) = (0.33, 0.7, -0.3);
/// The angular width of the arcs the dark bevel lights round each end of its axis, and the exponent on their falloff:
/// two arcs this wide meet at the sides, where each gives about a third of its light.
pub const LARGE_PANEL_RIM_SPREAD_DARK: f32 = 4.0 * std::f32::consts::FRAC_PI_3;
pub const LARGE_PANEL_RIM_CURVATURE_DARK: f32 = 1.5;
/// The soft band of light under the dark bevel, along the top and the bottom: as many times wider than the bevel, on
/// arcs half as wide, at this share of its strength, so it also lifts the face where the blur of the darker backdrop
/// outside the panel darkens it near the edge.
pub const LARGE_PANEL_GLOW_DARK: GlowDiffuse = GlowDiffuse {
    height_scale: 13.0,
    spread_scale: 0.5,
    amount_scale: 0.125,
};
/// The colour the dark bevel mixes the face towards: white.
const TOWARDS_WHITE_MATRIX: ColorMatrix = [
    -1.0, 0.0, 0.0, 0.0, 1.0, //
    0.0, -1.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, -1.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The dark large panel's tone compression: saturation, white and black levels and luma compression. A bright spot in
/// the scene (a sunlit sky, a white page, the light centre of a picture) comes through as a light patch near the
/// panel's tint rather than a glare: white comes out near 125, black near 53.
pub const LARGE_PANEL_SATURATION_DARK: f32 = 0.93;
pub const LARGE_PANEL_WHITE_DARK: f32 = 0.94;
pub const LARGE_PANEL_BLACK_DARK: f32 = 0.164;
pub const LARGE_PANEL_MAX_LUMA_DARK: f32 = 0.415;
/// How much wider the large panel blurs than regular glass: its blur levels and its blur fill (10 and 16 points
/// rather than 5 and 8), so stripes, text and bright spots in the scene behind a launcher-sized sheet melt into broad
/// fields of colour instead of showing through as ripples.
pub const LARGE_PANEL_BLUR_SCALE: f32 = 2.0;
/// The large panel's drop shadow: how far below the panel it falls and its blur radius in points (its edge fades over
/// three radii), and its black's opacity in each appearance.
pub const LARGE_PANEL_SHADOW_OFFSET: f32 = 16.0;
pub const LARGE_PANEL_SHADOW_BLUR: f32 = 24.0;
pub const LARGE_PANEL_SHADOW_DARK: f32 = 0.5;
pub const LARGE_PANEL_SHADOW_LIGHT: f32 = 0.40;

/// The drop shadow of an alert's regular glass: its blur radius and how far below the alert it falls in points, and its
/// black's opacity in the light and the dark appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlertShadow {
    pub blur: f32,
    pub offset: f32,
    pub light: f32,
    pub dark: f32,
}

/// The alert's shadow while it has the keyboard, and the shallower, fainter one while another window has it.
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

mod recipes;
pub use recipes::*;
