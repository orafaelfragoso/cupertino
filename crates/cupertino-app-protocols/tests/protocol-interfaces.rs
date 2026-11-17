//! The generated bindings expose the protocols' interfaces under their wire names and versions.

#[cfg(feature = "client")]
#[test]
fn client_interfaces_have_wire_names_and_versions() {
    use cupertino_app_protocols::client::{app_effects, layers, scroll, semantics};
    use wayland_client::Proxy;
    let got = [
        (
            scroll::cupertino_scroll_v1::CupertinoScrollV1::interface(),
            "cupertino_scroll_v1",
            1,
        ),
        (
            scroll::cupertino_pointer_scroll_v1::CupertinoPointerScrollV1::interface(),
            "cupertino_pointer_scroll_v1",
            1,
        ),
        (
            semantics::cupertino_semantics_v1::CupertinoSemanticsV1::interface(),
            "cupertino_semantics_v1",
            2,
        ),
        (
            semantics::cupertino_semantic_surface_v1::CupertinoSemanticSurfaceV1::interface(),
            "cupertino_semantic_surface_v1",
            2,
        ),
        (
            layers::cupertino_layers_v1::CupertinoLayersV1::interface(),
            "cupertino_layers_v1",
            6,
        ),
        (
            layers::cupertino_layer_v1::CupertinoLayerV1::interface(),
            "cupertino_layer_v1",
            6,
        ),
        (
            app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1::interface(),
            "cupertino_app_effects_v1",
            21,
        ),
        (
            app_effects::cupertino_backdrop_v1::CupertinoBackdropV1::interface(),
            "cupertino_backdrop_v1",
            20,
        ),
        (
            app_effects::cupertino_appearance_v1::CupertinoAppearanceV1::interface(),
            "cupertino_appearance_v1",
            21,
        ),
        (
            app_effects::cupertino_app_menu_v1::CupertinoAppMenuV1::interface(),
            "cupertino_app_menu_v1",
            3,
        ),
    ];
    for (iface, name, version) in got {
        assert_eq!((iface.name, iface.version), (name, version));
    }
}

#[cfg(feature = "server")]
#[test]
fn server_interfaces_have_wire_names_and_versions() {
    use cupertino_app_protocols::server::{app_effects, layers, scroll, semantics};
    use wayland_server::Resource;
    let got = [
        (
            scroll::cupertino_scroll_v1::CupertinoScrollV1::interface(),
            "cupertino_scroll_v1",
            1,
        ),
        (
            scroll::cupertino_pointer_scroll_v1::CupertinoPointerScrollV1::interface(),
            "cupertino_pointer_scroll_v1",
            1,
        ),
        (
            semantics::cupertino_semantics_v1::CupertinoSemanticsV1::interface(),
            "cupertino_semantics_v1",
            2,
        ),
        (
            semantics::cupertino_semantic_surface_v1::CupertinoSemanticSurfaceV1::interface(),
            "cupertino_semantic_surface_v1",
            2,
        ),
        (
            layers::cupertino_layers_v1::CupertinoLayersV1::interface(),
            "cupertino_layers_v1",
            6,
        ),
        (
            layers::cupertino_layer_v1::CupertinoLayerV1::interface(),
            "cupertino_layer_v1",
            6,
        ),
        (
            app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1::interface(),
            "cupertino_app_effects_v1",
            21,
        ),
        (
            app_effects::cupertino_backdrop_v1::CupertinoBackdropV1::interface(),
            "cupertino_backdrop_v1",
            20,
        ),
        (
            app_effects::cupertino_appearance_v1::CupertinoAppearanceV1::interface(),
            "cupertino_appearance_v1",
            21,
        ),
        (
            app_effects::cupertino_app_menu_v1::CupertinoAppMenuV1::interface(),
            "cupertino_app_menu_v1",
            3,
        ),
    ];
    for (iface, name, version) in got {
        assert_eq!((iface.name, iface.version), (name, version));
    }
}
