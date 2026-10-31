use crate::{FontStyle, FontWeight};
use std::collections::HashSet;
use std::ffi::CString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

/// The largest font file read from the system.
const MAX_FONT_BYTES: u64 = 64 << 20;

/// One face of a font file: the file and the face's index in it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontFile {
    pub path: PathBuf,
    pub index: u32,
}

/// The fonts installed on the system, asked for a face only where the registered faces have none.
pub trait SystemFonts: Send {
    /// A face of the installed family `family`; `None` when no installed family has that name.
    fn family(&self, family: &str, weight: FontWeight, style: FontStyle) -> Option<FontFile>;
    /// A face that draws `ch`, the one the system prefers for the language `lang` (a BCP 47 tag) where several do.
    fn covering(
        &self,
        ch: char,
        lang: Option<&str>,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<FontFile>;
}

/// The system's fonts through Fontconfig, loaded at run time: the user's and the distribution's font configuration
/// choose the face for a script or language.
pub struct InstalledFonts {
    fc: fontconfig::Fontconfig,
}

impl InstalledFonts {
    /// `None` when the Fontconfig library is missing or does not start.
    pub fn new() -> Option<Self> {
        fontconfig::Fontconfig::new().map(|fc| InstalledFonts { fc })
    }

    /// The best match for `pattern`, with its family.
    fn best(&self, pattern: &mut fontconfig::Pattern<'_>) -> Option<(FontFile, String)> {
        let found = pattern.font_match().ok()?;
        Some((
            FontFile {
                path: PathBuf::from(found.filename().ok()?),
                index: u32::try_from(found.face_index().ok()?).ok()?,
            },
            found.get_string(c"family").ok()?.to_owned(),
        ))
    }

    fn pattern(&self, weight: FontWeight, style: FontStyle) -> Option<fontconfig::Pattern<'_>> {
        let mut pattern = fontconfig::Pattern::new(&self.fc).ok()?;
        pattern.add_integer(c"weight", fc_weight(weight)).ok()?;
        pattern
            .add_integer(
                c"slant",
                match style {
                    FontStyle::Normal => 0,
                    FontStyle::Italic => 100,
                },
            )
            .ok()?;
        Some(pattern)
    }
}

impl SystemFonts for InstalledFonts {
    fn family(&self, family: &str, weight: FontWeight, style: FontStyle) -> Option<FontFile> {
        let name = CString::new(family).ok()?;
        let mut pattern = self.pattern(weight, style)?;
        pattern.add_string(c"family", &name).ok()?;
        // Fontconfig answers every family with its default face; only a face of that family counts
        let (file, found) = self.best(&mut pattern)?;
        found.eq_ignore_ascii_case(family).then_some(file)
    }

    fn covering(
        &self,
        ch: char,
        lang: Option<&str>,
        weight: FontWeight,
        style: FontStyle,
    ) -> Option<FontFile> {
        // Fontconfig's best match is only the nearest face, which can lack the character: take the nearest face of the
        // ones whose character set has it
        let mut has = fontconfig::Pattern::new(&self.fc).ok()?;
        has.add_charset(self.chars(ch)?).ok()?;
        let mut objects = fontconfig::ObjectSet::new(&self.fc).ok()?;
        objects.add(c"file").ok()?;
        objects.add(c"index").ok()?;
        let covering: HashSet<FontFile> = fontconfig::list_fonts(&has, Some(&objects))
            .ok()?
            .iter()
            .filter_map(|found| file_of(&found))
            .collect();
        if covering.is_empty() {
            return None;
        }
        let mut pattern = self.pattern(weight, style)?;
        pattern.add_charset(self.chars(ch)?).ok()?;
        if let Some(lang) = lang.and_then(|lang| CString::new(lang).ok()) {
            pattern.add_string(c"lang", &lang).ok()?;
        }
        pattern.config_substitute().ok()?;
        pattern.default_substitute();
        let nearest = pattern
            .sort_fonts(fontconfig::UnicodeCoverage::NoTrim)
            .ok()?
            .iter()
            .filter_map(|found| file_of(&found))
            .find(|file| covering.contains(file));
        nearest.or_else(|| covering.into_iter().min_by(|a, b| a.path.cmp(&b.path)))
    }
}

impl InstalledFonts {
    fn chars(&self, ch: char) -> Option<fontconfig::CharSet> {
        let mut chars = fontconfig::CharSet::new(&self.fc).ok()?;
        chars.add_char(ch).ok()?;
        Some(chars)
    }
}

fn file_of(found: &fontconfig::Pattern<'_>) -> Option<FontFile> {
    Some(FontFile {
        path: PathBuf::from(found.filename().ok()?),
        index: u32::try_from(found.face_index().ok()?).ok()?,
    })
}

/// Fontconfig's weight scale for an OpenType weight.
fn fc_weight(weight: FontWeight) -> i32 {
    const SCALE: [(u16, i32); 9] = [
        (100, 0),
        (200, 40),
        (300, 50),
        (400, 80),
        (500, 100),
        (600, 180),
        (700, 200),
        (800, 205),
        (900, 210),
    ];
    SCALE
        .iter()
        .min_by_key(|(css, _)| css.abs_diff(weight.0))
        .map_or(80, |(_, fc)| *fc)
}

/// The bytes of a font file: a regular file of at most 64 MiB.
pub(crate) fn read_font(path: &Path) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file of at most 64 MiB",
        ));
    }
    let mut data = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FONT_BYTES).read_to_end(&mut data)?;
    Ok(data)
}
