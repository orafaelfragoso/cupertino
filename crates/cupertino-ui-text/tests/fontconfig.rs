//! `InstalledFonts` against Fontconfig configured with the test fonts alone (`FONTCONFIG_FILE`), so the answers do not
//! depend on the fonts of the machine.

use cupertino_ui_text::{FontStyle, FontWeight, InstalledFonts, SystemFonts};
use std::path::{Path, PathBuf};

fn installed(cache: &Path) -> Option<InstalledFonts> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = cache.join("fonts.conf");
    std::fs::write(
        &config,
        format!(
            "<?xml version=\"1.0\"?>\n<fontconfig>\n<dir>{}</dir>\n<dir>{}</dir>\n<cachedir>{}</cachedir>\n</fontconfig>\n",
            crate_dir.join("tests/fixtures").display(),
            crate_dir.join("../cupertino-assets/fonts").display(),
            cache.display(),
        ),
    )
    .unwrap();
    std::env::set_var("FONTCONFIG_FILE", &config);
    let installed = InstalledFonts::new();
    if installed.is_none() {
        eprintln!("skipped: the Fontconfig library is not installed");
    }
    installed
}

fn file_name(path: PathBuf) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

#[test]
fn a_character_takes_a_face_that_has_it_and_one_no_face_has_takes_none() {
    let cache = tempfile::tempdir().unwrap();
    let Some(installed) = installed(cache.path()) else {
        return;
    };
    let covering = |ch, lang| {
        installed
            .covering(ch, lang, FontWeight::REGULAR, FontStyle::Normal)
            .map(|file| file_name(file.path))
    };
    assert_eq!(
        covering('\u{5D0}', Some("hi")).as_deref(),
        Some("noto-hebrew-subset.ttf"),
        "the language asks for Devanagari, but only the Hebrew face has alef"
    );
    assert_eq!(
        covering('\u{915}', None).as_deref(),
        Some("noto-devanagari-subset.ttf")
    );
    assert_eq!(
        covering('\u{2603}', None),
        None,
        "the nearest face to a character no face has is not an answer"
    );
}

#[test]
fn a_family_is_found_by_its_name_and_no_other_family_stands_in() {
    let cache = tempfile::tempdir().unwrap();
    let Some(installed) = installed(cache.path()) else {
        return;
    };
    let family = |name| {
        installed
            .family(name, FontWeight::REGULAR, FontStyle::Normal)
            .map(|file| file_name(file.path))
    };
    assert_eq!(
        family("Noto Sans Arabic").as_deref(),
        Some("noto-arabic-subset.ttf")
    );
    assert_eq!(family("No Such Family"), None);
}
