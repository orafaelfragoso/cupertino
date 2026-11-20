//! What every Kit instance of a process shares (D5), made once by the first instance that starts: the installed theme
//! data, which never changes once read, and the text system with the asset packs' fonts and the packs' icons, whose
//! caches only take whole entries that never change afterwards, under locks held only to look one up or put one in. A
//! panic in one instance therefore never leaves any of them half-changed for the others; nothing that differs between
//! instances lives here.

use crate::icons::PackIcons;
use cupertino_assets::{Assets, FontRole, FontSource, BUILTIN_FONT};
use cupertino_theme::Theme;
use cupertino_ui_text::{FontData, FontIndex, FontStyle, FontWeight, InstalledFonts, TextSystem};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// The theme data, the text system and the icons of the process's Kit instances.
pub(crate) struct Shared {
    /// The installed theme data with its material recipes, or why it could not be read.
    pub(crate) theme: Result<Arc<Theme>, String>,
    /// The asset packs' fonts, the built-in Inter and the installed fonts, with the caches of shaped paragraphs and
    /// glyph rasters every instance's windows lay out and draw with.
    pub(crate) text: Arc<TextSystem>,
    /// The asset packs' symbols and the running program's icon.
    pub(crate) icons: Arc<PackIcons>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

impl Shared {
    /// The process's, made by the first caller.
    pub(crate) fn get() -> &'static Shared {
        SHARED.get_or_init(Shared::load)
    }

    /// The process's once an instance made it.
    pub(crate) fn made() -> Option<&'static Shared> {
        SHARED.get()
    }

    /// Reads the theme data and the asset packs. UI text is set in the packs' interface family, else in the theme
    /// data's, else in the built-in Inter.
    fn load() -> Self {
        let theme = cupertino_assets::theme::load()
            .map(Arc::new)
            .map_err(|e| e.to_string());
        let assets = Assets::discover();
        let (fonts, builtin) = fonts(&assets);
        let theme_family = theme.as_ref().ok().map(|t| t.tokens.typography().family);
        let preferred = [assets.font_family(FontRole::Ui), theme_family];
        let family = ui_family(&fonts, preferred.into_iter().flatten(), builtin.as_deref());
        Shared {
            theme,
            text: Arc::new(TextSystem::new(fonts, &family)),
            icons: Arc::new(PackIcons::new(&assets)),
        }
    }
}

/// The largest font file read; the biggest real collections are a few tens of megabytes.
const MAX_FONT_BYTES: u64 = 64 << 20;

/// A regular file of at most [`MAX_FONT_BYTES`], so a pack entry that names a device, a pipe or a huge file never
/// stalls or exhausts an application at start.
fn read_font(path: &Path) -> io::Result<Vec<u8>> {
    let too_big = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file of at most 64 MiB",
        )
    };
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(too_big());
    }
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(too_big());
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FONT_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_FONT_BYTES {
        return Err(too_big());
    }
    Ok(data)
}

/// The first of `preferred` that names a loaded face, else `builtin`: UI text never falls to whichever face happened
/// to load first.
fn ui_family<'a>(
    fonts: &FontIndex,
    mut preferred: impl Iterator<Item = &'a str>,
    builtin: Option<&'a str>,
) -> String {
    preferred
        .find(|family| {
            fonts
                .match_face(family, FontWeight::REGULAR, FontStyle::Normal)
                .is_some()
        })
        .or(builtin)
        .unwrap_or_default()
        .to_owned()
}

/// The asset packs' fonts, then the built-in Inter, with the family Inter registered under, and the installed fonts
/// for the characters and families none of them has. A file that cannot be read or parsed is skipped with a message.
fn fonts(assets: &Assets) -> (FontIndex, Option<String>) {
    let mut index = FontIndex::new();
    let mut builtin = None;
    for source in assets.fonts() {
        let data: FontData = match &source {
            FontSource::File(path) => match read_font(path) {
                Ok(data) => Arc::new(data),
                Err(e) => {
                    eprintln!("cupertino-kit: skipping the font {}: {e}", path.display());
                    continue;
                }
            },
            // the program's own bytes, registered without a copy
            FontSource::Builtin => Arc::new(BUILTIN_FONT),
        };
        match index.add_font(data) {
            Ok(faces) if matches!(source, FontSource::Builtin) => {
                builtin = faces
                    .first()
                    .and_then(|&face| index.family(face))
                    .map(|family| family.to_string());
            }
            Ok(_) => {}
            Err(e) => eprintln!("cupertino-kit: skipping the font {}: {e:?}", source.label()),
        }
    }
    if let Some(installed) = InstalledFonts::new() {
        index.set_system_fonts(Box::new(installed));
    }
    (index, builtin)
}
