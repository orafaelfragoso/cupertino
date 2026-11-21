//! What a program that draws installed applications' and files' icons itself reads through `kit::shared`, from any
//! thread of the process: the installed file types, a host icon theme's icon (here a file it names) and the asset
//! packs' raster icons, each read the first time and the same for every caller after, so an instance started again
//! after a panic finds what the ones before it read.

use cupertino_kit::shared;
use std::path::Path;

/// The file name pattern of the test's own file type.
const PATTERN: &str = "*.cupertest";

/// Writes the shared MIME database of `data`, `pattern` typed as `mime`.
fn types(data: &Path, mime: &str) {
    std::fs::create_dir_all(data.join("mime")).unwrap();
    std::fs::write(data.join("mime/globs2"), format!("50:{mime}:{PATTERN}\n")).unwrap();
}

/// The type the database gives a file of the test's own type.
fn typed(types: &shared::MimeDatabase) -> String {
    types.file_type(Path::new("a.cupertest"), false).to_owned()
}

/// What `work` gives on a thread of its own.
fn elsewhere<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(work).join().unwrap()
}

#[test]
fn file_types_and_icons_are_read_once_for_the_process() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("shared-installed");
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    types(&data, "text/x-cupertino-first");
    // no asset pack: every raster icon is the placeholder
    std::env::set_var(cupertino_assets::SEARCH_PATH_ENV, "");
    std::env::set_var("HOME", &root);
    std::env::set_var("XDG_DATA_HOME", &data);
    std::env::set_var("XDG_DATA_DIRS", &data);

    assert_eq!(typed(shared::file_types()), "text/x-cupertino-first");
    types(&data, "text/x-cupertino-second");
    assert!(std::ptr::eq(
        elsewhere(shared::file_types),
        shared::file_types()
    ));
    assert_eq!(
        elsewhere(|| typed(shared::file_types())),
        "text/x-cupertino-first",
        "read once"
    );

    let red = root.join("red.svg");
    std::fs::write(
        &red,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#ff0000"/></svg>"##,
    )
    .unwrap();
    let path = red.to_str().unwrap().to_owned();
    let icon = shared::theme_icon(&path, 24).expect("the icon of the file it names");
    assert_eq!((icon.image.width, icon.image.height), (24, 24));
    assert!(icon
        .image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| *p == [255, 0, 0, 255]));
    let again = elsewhere(move || shared::theme_icon(&path, 24).unwrap());
    assert_eq!(again.id, icon.id, "drawn once");
    assert!(shared::theme_icon("org.cupertino.NoSuchApplication", 24).is_none());

    // (name, given by a pack, an image)
    for (name, given, image) in [
        ("trash.full", false, true),
        ("folder.downloads", false, true),
        ("not an icon name", false, false),
    ] {
        assert_eq!(shared::pack_has_raster(name, 32), given, "{name}");
        let raster = shared::pack_raster(name, 32);
        assert_eq!(raster.is_some(), image, "{name}");
        if let Some(raster) = raster {
            assert_eq!(
                (raster.image.width, raster.image.height),
                (32, 32),
                "{name}"
            );
            let again = elsewhere(move || shared::pack_raster(name, 32).unwrap());
            assert_eq!(again.id, raster.id, "{name} drawn once");
        }
    }
}
