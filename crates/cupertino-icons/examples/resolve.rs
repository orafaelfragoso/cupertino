//! Prints the file each application icon name resolves to on this machine (the asset directory's `application.`
//! names, the `hicolor` theme and the flat directories only), at 16 pixels, scale 1. Needs `CUPERTINO_ASSET_DIR`.

use cupertino_icons::IconLookup;

fn main() -> Result<(), cupertino_assets::AssetError> {
    let mut lookup = IconLookup::new(&cupertino_assets::Assets::discover());
    for name in std::env::args().skip(1) {
        match lookup.find(&name, 16, 1) {
            Some(path) => println!("{name}: {}", path.display()),
            None => println!("{name}: not found"),
        }
    }
    Ok(())
}
