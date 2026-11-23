//! The toolkit's system glass variants as the protocol names them (`cupertino_backdrop_v1.set_glass_variant`,
//! `cupertino_layer_v1.set_glass_variant`).

use cupertino_ui_core::platform::{GlassModifiers, GlassSize, GlassVariant};
use cupertino_wayland_client::app::{
    GlassModifiers as WireModifiers, GlassSizeClass, GlassVariant as Wire, VariantGlass,
};

/// The protocol's glass of a system `variant` and its modifiers, a document-modal sheet's when `document_modal`; the
/// compositor takes their `active` from the window.
pub(crate) fn protocol_variant(
    (variant, modifiers): (GlassVariant, GlassModifiers),
    document_modal: bool,
) -> VariantGlass {
    VariantGlass {
        variant: wire_variant(variant),
        size: match modifiers.size {
            GlassSize::Dynamic => GlassSizeClass::Dynamic,
            GlassSize::Minimum => GlassSizeClass::Minimum,
            GlassSize::Medium => GlassSizeClass::Medium,
            GlassSize::Large => GlassSizeClass::Large,
        },
        modifiers: WireModifiers {
            subdued: modifiers.subdued,
            exclude_shadow: modifiers.exclude_shadow,
            interactive: modifiers.interactive,
            adaptive: modifiers.adaptive,
            document_modal,
        },
    }
}

/// The protocol's name of a system glass variant.
fn wire_variant(variant: GlassVariant) -> Wire {
    match variant {
        GlassVariant::Regular => Wire::Regular,
        GlassVariant::Clear => Wire::Clear,
        GlassVariant::Dock => Wire::Dock,
        GlassVariant::AppIcons => Wire::AppIcons,
        GlassVariant::ControlCenter => Wire::ControlCenter,
        GlassVariant::NotificationCenter => Wire::NotificationCenter,
        GlassVariant::Sidebar => Wire::Sidebar,
        GlassVariant::AbuttedSidebar => Wire::AbuttedSidebar,
        GlassVariant::Inspector => Wire::Inspector,
        GlassVariant::CartouchePopover => Wire::CartouchePopover,
        GlassVariant::Menu => Wire::Menu,
        GlassVariant::Tab => Wire::Tab,
        GlassVariant::GlassWindow => Wire::GlassWindow,
        GlassVariant::Control => Wire::Control,
        GlassVariant::Slider => Wire::Slider,
        GlassVariant::Loupe => Wire::Loupe,
        GlassVariant::SearchResults => Wire::SearchResults,
        GlassVariant::AppSwitcher => Wire::AppSwitcher,
        GlassVariant::FocusBorder => Wire::FocusBorder,
        GlassVariant::EntryField => Wire::EntryField,
        GlassVariant::ToolbarItem => Wire::ToolbarItem,
        GlassVariant::Identity => Wire::Identity,
    }
}
