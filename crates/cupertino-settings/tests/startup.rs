//! Missing template symbols do not stop the start (ST-20): the painter is made from an asset pack that provides none of
//! the sidebar's symbols and no font, so it draws with the built-in font and placeholder symbols.

use cupertino_assets::Assets;
use cupertino_settings::paint::Painter;

#[test]
fn a_missing_symbol_does_not_stop_the_painter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{"version": 2, "name": "empty"}"#,
    )
    .unwrap();
    let assets = Assets::from_dirs(&[dir.path().to_path_buf()]);
    assert!(Painter::new(&assets).is_ok());
}
