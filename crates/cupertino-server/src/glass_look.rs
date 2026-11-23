//! The glass a backdrop (`cupertino_backdrop_v1`) or a layer (`cupertino_layer_v1`) asks for: a legacy style of
//! `set_glass`, or a system variant with its modifiers and size class of `set_glass_variant` (backdrop version 23,
//! layer version 7), and how either resolves to a [`GlassMode`] with the window's activation. The wire values of the
//! variants, the modifier bits and the size classes are those of `cupertino_backdrop_v1`, which the layers protocol
//! names too.

use cupertino_protocols::server::app_effects::cupertino_backdrop_v1::{
    GlassModifier as WireModifier, GlassVariant as WireVariant, SizeClass as WireSize,
};
use cupertino_theme::{GlassMode, GlassModifiers, GlassSize, GlassStyle, GlassVariant};

/// The legacy styles in the order of their wire values (`cupertino_backdrop_v1.glass_style`).
pub const GLASS_STYLES: [GlassStyle; 16] = [
    GlassStyle::Regular,
    GlassStyle::Clear,
    GlassStyle::LargePanel,
    GlassStyle::Bar,
    GlassStyle::Dense,
    GlassStyle::Module,
    GlassStyle::Card,
    GlassStyle::Track,
    GlassStyle::Popover,
    GlassStyle::Switcher,
    GlassStyle::Sidebar,
    GlassStyle::SubduedModule,
    GlassStyle::Snippet,
    GlassStyle::Veiled,
    GlassStyle::Toolbar,
    GlassStyle::Menu,
];

/// The share of a client's tint alpha laid over the face of its glass ([`cupertino_theme::GlassParams::with_tint`]):
/// all of it, so the alpha the client sends is the tint's strength.
pub const TINT_FACE_OPACITY: f32 = 1.0;

/// What glass a backdrop or a layer asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlassLook {
    /// The legacy style `set_glass` named, or for a variant the legacy style that draws as it (the same variant and
    /// modifiers), if one does: the server's own adjustments of a style's glass key on it, so a variant draws as its
    /// style does.
    pub style: Option<GlassStyle>,
    pub variant: GlassVariant,
    /// The modifiers asked for; [`GlassModifiers::active`] is the window's, set when the glass is drawn.
    pub modifiers: GlassModifiers,
    /// Asked for as a variant: the glass takes the inactive branch while its window is inactive. Glass of a legacy
    /// style keeps the active recipe, as it always drew.
    pub follows_window: bool,
}

impl GlassLook {
    /// The glass of legacy `style`, as [`GlassStyle::variant`] maps it.
    pub fn of_style(style: GlassStyle) -> Self {
        let (variant, modifiers) = style.variant();
        Self {
            style: Some(style),
            variant,
            modifiers,
            follows_window: false,
        }
    }

    /// The glass of `variant` with `modifiers`, which follows its window's activation.
    pub fn of_variant(variant: GlassVariant, modifiers: GlassModifiers) -> Self {
        // the modifiers a style's recipe does not name: carried, not part of what it draws
        let drawn = GlassModifiers {
            interactive: false,
            adaptive: false,
            active: true,
            ..modifiers
        };
        Self {
            style: GLASS_STYLES
                .into_iter()
                .find(|s| s.variant() == (variant, drawn)),
            variant,
            modifiers: GlassModifiers {
                active: true,
                ..modifiers
            },
            follows_window: true,
        }
    }

    /// `base` (appearance and display options) with this glass's variant and modifiers, in a window that is `active`
    /// or not.
    pub fn mode(self, base: GlassMode, active: bool) -> GlassMode {
        GlassMode {
            variant: self.variant,
            modifiers: GlassModifiers {
                active: active || !self.follows_window,
                ..self.modifiers
            },
            ..base
        }
    }

    /// Whether the glass responds to a press ([`crate::glass_press`]).
    pub fn interactive(self) -> bool {
        self.modifiers.interactive
    }
}

/// The variant of a wire value, `None` for one the protocol does not define.
pub fn variant(wire: u32) -> Option<GlassVariant> {
    Some(match WireVariant::try_from(wire).ok()? {
        WireVariant::Regular => GlassVariant::Regular,
        WireVariant::Clear => GlassVariant::Clear,
        WireVariant::Dock => GlassVariant::Dock,
        WireVariant::AppIcons => GlassVariant::AppIcons,
        WireVariant::ControlCenter => GlassVariant::ControlCenter,
        WireVariant::NotificationCenter => GlassVariant::NotificationCenter,
        WireVariant::Sidebar => GlassVariant::Sidebar,
        WireVariant::AbuttedSidebar => GlassVariant::AbuttedSidebar,
        WireVariant::Inspector => GlassVariant::Inspector,
        WireVariant::CartouchePopover => GlassVariant::CartouchePopover,
        WireVariant::Menu => GlassVariant::Menu,
        WireVariant::Tab => GlassVariant::Tab,
        WireVariant::GlassWindow => GlassVariant::GlassWindow,
        WireVariant::Control => GlassVariant::Control,
        WireVariant::Slider => GlassVariant::Slider,
        WireVariant::Loupe => GlassVariant::Loupe,
        WireVariant::SearchResults => GlassVariant::SearchResults,
        WireVariant::AppSwitcher => GlassVariant::AppSwitcher,
        WireVariant::FocusBorder => GlassVariant::FocusBorder,
        WireVariant::EntryField => GlassVariant::EntryField,
        WireVariant::ToolbarItem => GlassVariant::ToolbarItem,
        WireVariant::Identity => GlassVariant::Identity,
        _ => return None,
    })
}

/// The modifiers of wire `bits` resolved at the size class `size`, `None` for a bit or a size class the protocol does
/// not define.
pub fn modifiers(bits: u32, size: u32) -> Option<GlassModifiers> {
    let flags = WireModifier::from_bits(bits)?;
    let size = match WireSize::try_from(size).ok()? {
        WireSize::Dynamic => GlassSize::Dynamic,
        WireSize::Minimum => GlassSize::Minimum,
        WireSize::Medium => GlassSize::Medium,
        WireSize::Large => GlassSize::Large,
        _ => return None,
    };
    Some(GlassModifiers {
        size,
        subdued: flags.contains(WireModifier::Subdued),
        exclude_shadow: flags.contains(WireModifier::ExcludeShadow),
        interactive: flags.contains(WireModifier::Interactive),
        adaptive: flags.contains(WireModifier::Adaptive),
        active: true,
    })
}

/// The glass of `set_glass_variant`'s wire values, `None` for one the protocol does not define.
pub fn of_wire(variant_wire: u32, bits: u32, size: u32) -> Option<GlassLook> {
    Some(GlassLook::of_variant(
        variant(variant_wire)?,
        modifiers(bits, size)?,
    ))
}

/// A tint of `set_glass_tint` as the scene lays it, straight sRGB; `None` for a channel below 0, above 1 or not
/// finite.
pub fn tint(channels: [f64; 4]) -> Option<[f32; 4]> {
    channels
        .iter()
        .all(|c| c.is_finite() && (0.0..=1.0).contains(c))
        .then(|| channels.map(|c| c as f32))
}
