//! The system's glass variants as `cupertino_backdrop_v1.set_glass_variant` (version 23) and
//! `cupertino_layer_v1.set_glass_variant` (version 7) name them: a variant, the size class its size-dependent values
//! resolve at and its modifiers ([`VariantGlass`]), and the legacy style a server before them draws instead.

use crate::app::GlassStyle;
use cupertino_app_protocols::client::app_effects::cupertino_backdrop_v1::{
    GlassModifier as WireModifier, GlassVariant as WireVariant, SizeClass as WireSize,
};

/// A glass variant of the system: a recipe built on the regular or the clear glass, with stages of its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassVariant {
    /// The regular glass of controls, bars and panels with text on them.
    #[default]
    Regular,
    /// The clear glass, nearly unblurred and untinted, for controls over media.
    Clear,
    /// The dock's bar.
    Dock,
    /// The glass of application icons.
    AppIcons,
    /// The modules of the control centre.
    ControlCenter,
    /// The cards of the notification centre.
    NotificationCenter,
    /// A floating sidebar.
    Sidebar,
    /// A window's sidebar against its leading edge.
    AbuttedSidebar,
    /// A window's inspector against its trailing edge.
    Inspector,
    /// A small popover: the dock's labels and its stack grid.
    CartouchePopover,
    /// A menu.
    Menu,
    /// A selected tab or toolbar segment.
    Tab,
    /// An alert or a sheet.
    GlassWindow,
    /// A control's lens, a held switch knob.
    Control,
    /// A slider knob's lens.
    Slider,
    /// The magnifying loupe.
    Loupe,
    /// The launcher's results panel.
    SearchResults,
    /// The application switcher.
    AppSwitcher,
    /// The glass behind a focus ring.
    FocusBorder,
    /// A text field.
    EntryField,
    /// A window toolbar's item capsule.
    ToolbarItem,
    /// No glass: what lies behind the shape, as it is.
    Identity,
}

/// The size a variant's size-dependent values (blur, bend, bleed) resolve at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum GlassSizeClass {
    /// The shape's own shorter side.
    #[default]
    Dynamic,
    /// The smallest size, a control's.
    Minimum,
    /// The middle size, a card's.
    Medium,
    /// The large size, where the regular blur is widest.
    Large,
}

/// What changes how a variant resolves, besides its size class and its window's activation (which the server knows).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GlassModifiers {
    /// The glass steps back behind what is in front of it.
    pub subdued: bool,
    /// No drop shadow from the glass: its surface draws its own.
    pub exclude_shadow: bool,
    /// The glass grows a little under a press and lets go on the release, on the server's clock.
    pub interactive: bool,
    /// The glass adapts to what lies behind it.
    pub adaptive: bool,
    /// The glass is a document-modal sheet: the glass window variant casts a sheet's drop shadow, fainter than an
    /// alert's.
    pub document_modal: bool,
}

/// Glass named by its system variant ([`crate::app::Backdrop::set_glass_variant`],
/// [`crate::app::Layer::set_glass_variant`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VariantGlass {
    pub variant: GlassVariant,
    pub size: GlassSizeClass,
    pub modifiers: GlassModifiers,
}

impl VariantGlass {
    /// `variant` at the shape's own size, with no modifier.
    pub fn new(variant: GlassVariant) -> Self {
        Self {
            variant,
            ..Self::default()
        }
    }

    /// The legacy style a server without variants draws in its place: the style that draws as the variant where one
    /// does, else the nearest.
    pub fn fallback_style(self) -> GlassStyle {
        let m = self.modifiers;
        match self.variant {
            GlassVariant::Regular
            | GlassVariant::Tab
            | GlassVariant::GlassWindow
            | GlassVariant::FocusBorder
            | GlassVariant::EntryField => GlassStyle::Regular,
            GlassVariant::Clear | GlassVariant::AppIcons | GlassVariant::Identity => {
                GlassStyle::Clear
            }
            GlassVariant::Dock => GlassStyle::Bar,
            GlassVariant::ControlCenter if m.subdued => GlassStyle::SubduedModule,
            GlassVariant::ControlCenter => GlassStyle::Module,
            GlassVariant::NotificationCenter if self.size == GlassSizeClass::Dynamic => {
                GlassStyle::Snippet
            }
            GlassVariant::NotificationCenter => GlassStyle::Card,
            GlassVariant::Sidebar | GlassVariant::AbuttedSidebar | GlassVariant::Inspector => {
                GlassStyle::Sidebar
            }
            GlassVariant::CartouchePopover if self.size == GlassSizeClass::Minimum => {
                GlassStyle::Popover
            }
            GlassVariant::CartouchePopover => GlassStyle::Dense,
            GlassVariant::Menu => GlassStyle::Menu,
            GlassVariant::Control | GlassVariant::Slider | GlassVariant::Loupe => GlassStyle::Track,
            GlassVariant::SearchResults => GlassStyle::LargePanel,
            GlassVariant::AppSwitcher => GlassStyle::Switcher,
            GlassVariant::ToolbarItem => GlassStyle::Toolbar,
        }
    }

    /// The variant as the wire carries it.
    pub(crate) fn wire_variant(self) -> WireVariant {
        match self.variant {
            GlassVariant::Regular => WireVariant::Regular,
            GlassVariant::Clear => WireVariant::Clear,
            GlassVariant::Dock => WireVariant::Dock,
            GlassVariant::AppIcons => WireVariant::AppIcons,
            GlassVariant::ControlCenter => WireVariant::ControlCenter,
            GlassVariant::NotificationCenter => WireVariant::NotificationCenter,
            GlassVariant::Sidebar => WireVariant::Sidebar,
            GlassVariant::AbuttedSidebar => WireVariant::AbuttedSidebar,
            GlassVariant::Inspector => WireVariant::Inspector,
            GlassVariant::CartouchePopover => WireVariant::CartouchePopover,
            GlassVariant::Menu => WireVariant::Menu,
            GlassVariant::Tab => WireVariant::Tab,
            GlassVariant::GlassWindow => WireVariant::GlassWindow,
            GlassVariant::Control => WireVariant::Control,
            GlassVariant::Slider => WireVariant::Slider,
            GlassVariant::Loupe => WireVariant::Loupe,
            GlassVariant::SearchResults => WireVariant::SearchResults,
            GlassVariant::AppSwitcher => WireVariant::AppSwitcher,
            GlassVariant::FocusBorder => WireVariant::FocusBorder,
            GlassVariant::EntryField => WireVariant::EntryField,
            GlassVariant::ToolbarItem => WireVariant::ToolbarItem,
            GlassVariant::Identity => WireVariant::Identity,
        }
    }

    /// The modifier bits as the wire carries them.
    pub(crate) fn wire_modifiers(self) -> WireModifier {
        let m = self.modifiers;
        let mut bits = WireModifier::empty();
        for (on, bit) in [
            (m.subdued, WireModifier::Subdued),
            (m.exclude_shadow, WireModifier::ExcludeShadow),
            (m.interactive, WireModifier::Interactive),
            (m.adaptive, WireModifier::Adaptive),
            (m.document_modal, WireModifier::DocumentModal),
        ] {
            if on {
                bits |= bit;
            }
        }
        bits
    }

    /// The size class as the wire carries it.
    pub(crate) fn wire_size(self) -> WireSize {
        match self.size {
            GlassSizeClass::Dynamic => WireSize::Dynamic,
            GlassSizeClass::Minimum => WireSize::Minimum,
            GlassSizeClass::Medium => WireSize::Medium,
            GlassSizeClass::Large => WireSize::Large,
        }
    }
}

/// Whether every channel of a tint lies from 0 to 1.
pub(crate) fn valid_tint(rgba: [f64; 4]) -> bool {
    rgba.iter()
        .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
}
