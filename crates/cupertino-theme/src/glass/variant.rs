//! The glass variants of the system, the modifiers that change how a variant resolves, and the legacy styles mapped onto
//! them.

use super::{GlassMode, GlassStyle};

/// The glass variants of the system: each names a recipe built on one of two bases ([`GlassBase`]), the regular glass or
/// the clear glass, plus the variant's own stages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassVariant {
    /// The regular glass: controls, bars and panels with text on them.
    #[default]
    Regular,
    /// The clear glass, nearly unblurred and untinted, for controls over media.
    Clear,
    /// The Dock bar: the clear base with a slightly wider blur, its own face and a bend outside its edge.
    Dock,
    /// The glass of application icons: no values are recorded, so it is the clear glass.
    AppIcons,
    /// The control centre's modules: the clear base with the module face, the same in both appearances.
    ControlCenter,
    /// The notification centre's cards: the regular glass resolved at the medium size.
    NotificationCenter,
    /// A floating sidebar: the regular glass with the sidebar's face.
    Sidebar,
    /// A window's sidebar against its leading edge: the regular glass made planar (no light, no ring, no bend outside
    /// the edge).
    AbuttedSidebar,
    /// A window's inspector against its trailing edge: the abutted sidebar's recipe at the other edge.
    Inspector,
    /// A small popover (a cartouche: the dock's labels and stack grid): the regular glass with a narrow fixed bend.
    CartouchePopover,
    /// A menu: the regular glass resolved at the menu's own size, with the window shadow of a menu.
    Menu,
    /// A selected tab or toolbar segment: the regular glass under a white fill.
    Tab,
    /// The glass of an alert or a sheet: the regular glass with a window's drop shadow.
    GlassWindow,
    /// A control's lens (a switch knob held): no blur, its own face and ring.
    Control,
    /// A slider knob's lens: the control glass bending harder, with a deeper ring.
    Slider,
    /// The magnifying loupe: the control glass with a lower, softer ring.
    Loupe,
    /// The launcher's results panel: no values are recorded, so it keeps the fitted large panel glass.
    SearchResults,
    /// The application switcher: the Dock's recipe with a wider blur.
    AppSwitcher,
    /// The glass behind a focus ring: no values are recorded, so it is the regular glass.
    FocusBorder,
    /// The glass of a text field: the regular glass with no blur.
    EntryField,
    /// A window toolbar's item capsule: the regular glass with the toolbar group's modifiers (a fifth of the blur, a
    /// fixed bend and the control border). A variant of its own rather than a modifier, as nothing else takes those
    /// values.
    ToolbarItem,
    /// No glass: the backdrop as it is.
    Identity,
}

/// The base recipe a variant is built on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlassBase {
    /// The regular glass, whose shared stages (Increase Contrast, Reduce Transparency, the inactive branch, the
    /// legibility amount) every variant on it takes.
    Regular,
    /// The clear glass, which has no inactive branch.
    Clear,
}

/// The edge of the window a sidebar's glass lies against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SidebarEdge {
    /// The leading edge: a sidebar.
    Leading,
    /// The trailing edge: an inspector.
    Trailing,
}

impl GlassVariant {
    /// Every variant.
    pub const ALL: [Self; 22] = [
        Self::Regular,
        Self::Clear,
        Self::Dock,
        Self::AppIcons,
        Self::ControlCenter,
        Self::NotificationCenter,
        Self::Sidebar,
        Self::AbuttedSidebar,
        Self::Inspector,
        Self::CartouchePopover,
        Self::Menu,
        Self::Tab,
        Self::GlassWindow,
        Self::Control,
        Self::Slider,
        Self::Loupe,
        Self::SearchResults,
        Self::AppSwitcher,
        Self::FocusBorder,
        Self::EntryField,
        Self::ToolbarItem,
        Self::Identity,
    ];

    /// The base recipe the variant is built on; `None` for [`GlassVariant::Identity`], which has no glass.
    pub fn base(self) -> Option<GlassBase> {
        match self {
            Self::Clear
            | Self::Dock
            | Self::AppIcons
            | Self::ControlCenter
            | Self::Control
            | Self::Slider
            | Self::Loupe
            | Self::AppSwitcher => Some(GlassBase::Clear),
            Self::Identity => None,
            _ => Some(GlassBase::Regular),
        }
    }

    /// Whether the variant has a branch of its own while its window or application is inactive
    /// ([`GlassModifiers::active`]): every variant on the regular base does; the clear base, the Dock, the control
    /// centre and the controls do not.
    pub fn has_inactive_branch(self) -> bool {
        self.base() == Some(GlassBase::Regular)
    }

    /// The edge of the window the variant lies against: the sidebars' leading edge, the inspector's trailing one.
    pub fn sidebar_edge(self) -> Option<SidebarEdge> {
        match self {
            Self::Sidebar | Self::AbuttedSidebar => Some(SidebarEdge::Leading),
            Self::Inspector => Some(SidebarEdge::Trailing),
            _ => None,
        }
    }

    /// Whether the variant draws glass at all: [`GlassVariant::Identity`] shows the backdrop as it is, so a renderer
    /// may skip it (its recipe draws the backdrop unchanged where it does not).
    pub fn draws_glass(self) -> bool {
        self != Self::Identity
    }
}

/// The size a recipe resolves its size-dependent values at (blur, luma cap, bend, bleed).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassSize {
    /// The smallest size: [`GLASS_SIZE_MINIMUM`].
    Minimum,
    /// The middle size: [`GLASS_SIZE_MEDIUM`].
    Medium,
    /// The large size: [`GLASS_SIZE_LARGE`].
    Large,
    /// The shape's own short side.
    #[default]
    Dynamic,
}

/// The short side in points the minimum size resolves at: the fallback dimension the regular recipe records (a
/// control's size, where the size-dependent values are at their smallest).
pub const GLASS_SIZE_MINIMUM: f32 = 48.0;
/// The short side in points the medium size resolves at: not recorded; where the regular bend reaches its cap (−60 pt)
/// and the dark luma cap its floor, as the notification cards bent before (inferred).
pub const GLASS_SIZE_MEDIUM: f32 = 120.0;
/// The short side in points the large size resolves at: not recorded; where the regular blur reaches its widest
/// (inferred).
pub const GLASS_SIZE_LARGE: f32 = 160.0;

impl GlassSize {
    /// The short side the recipe resolves at, for a shape whose short side is `short_side` points.
    pub fn side(self, short_side: f32) -> f32 {
        match self {
            Self::Minimum => GLASS_SIZE_MINIMUM,
            Self::Medium => GLASS_SIZE_MEDIUM,
            Self::Large => GLASS_SIZE_LARGE,
            Self::Dynamic => short_side,
        }
    }
}

/// What changes how a variant resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlassModifiers {
    /// The size the recipe resolves at.
    pub size: GlassSize,
    /// The variant steps back behind what is in front of it: only the control centre has a subdued recipe (fitted to
    /// captures); elsewhere it is carried only.
    pub subdued: bool,
    /// No drop shadow from the glass: its surface draws its own (a menu's popup draws the window shadow apart).
    pub exclude_shadow: bool,
    /// The glass reacts to the pointer. Carried only: its motion is not part of the recipe.
    pub interactive: bool,
    /// The glass adapts to what lies behind it. Carried only.
    pub adaptive: bool,
    /// The window or application is active; `false` selects the inactive branch of a variant that has one
    /// ([`GlassVariant::has_inactive_branch`]).
    pub active: bool,
}

impl GlassModifiers {
    /// The modifiers of glass at the shape's own size, active, with nothing else changed.
    pub const DEFAULT: Self = Self {
        size: GlassSize::Dynamic,
        subdued: false,
        exclude_shadow: false,
        interactive: false,
        adaptive: false,
        active: true,
    };

    /// These modifiers resolved at `size`.
    pub fn sized(self, size: GlassSize) -> Self {
        Self { size, ..self }
    }
}

impl Default for GlassModifiers {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl GlassStyle {
    /// The system variant and modifiers this legacy style draws as. Snippet glass (a small card of content over the
    /// desktop) draws as the notification centre's cards at its own size, and veiled glass as clear glass: no variant
    /// records a veil, so the veil goes.
    pub fn variant(self) -> (GlassVariant, GlassModifiers) {
        let m = GlassModifiers::DEFAULT;
        match self {
            Self::Regular => (GlassVariant::Regular, m),
            Self::Clear | Self::Veiled => (GlassVariant::Clear, m),
            Self::LargePanel => (GlassVariant::SearchResults, m),
            Self::Bar => (GlassVariant::Dock, m),
            Self::Dense => (GlassVariant::CartouchePopover, m.sized(GlassSize::Medium)),
            Self::Module => (GlassVariant::ControlCenter, m),
            Self::SubduedModule => (
                GlassVariant::ControlCenter,
                GlassModifiers { subdued: true, ..m },
            ),
            Self::Card => (GlassVariant::NotificationCenter, m.sized(GlassSize::Medium)),
            Self::Snippet => (GlassVariant::NotificationCenter, m),
            Self::Track => (GlassVariant::Slider, m),
            Self::Popover => (GlassVariant::CartouchePopover, m.sized(GlassSize::Minimum)),
            Self::Switcher => (GlassVariant::AppSwitcher, m),
            Self::Sidebar => (GlassVariant::AbuttedSidebar, m),
            Self::Toolbar => (GlassVariant::ToolbarItem, m),
            // the popup draws the menu's window shadow apart
            Self::Menu => (
                GlassVariant::Menu,
                GlassModifiers {
                    exclude_shadow: true,
                    ..m
                },
            ),
        }
    }
}

impl GlassMode {
    /// Glass of `variant` with `modifiers`, in the dark appearance or the light one, with no display option.
    pub fn new(dark: bool, variant: GlassVariant, modifiers: GlassModifiers) -> Self {
        Self {
            dark,
            variant,
            modifiers,
            ..Self::default()
        }
    }

    /// Glass of the legacy `style` ([`GlassStyle::variant`]), with no display option.
    pub fn of_style(dark: bool, style: GlassStyle) -> Self {
        Self::default().with_style(style).appearance(dark)
    }

    /// This mode with the variant and modifiers of the legacy `style`, the appearance and options kept.
    pub fn with_style(self, style: GlassStyle) -> Self {
        let (variant, modifiers) = style.variant();
        Self {
            variant,
            modifiers,
            ..self
        }
    }

    /// This mode in the dark appearance (`dark`) or the light one.
    pub fn appearance(self, dark: bool) -> Self {
        Self { dark, ..self }
    }
}
