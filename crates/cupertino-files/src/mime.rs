//! MIME detection with the shared MIME database: by extension first (cheap, no I/O), by content
//! sniffing when the extension is unknown, and the "kind" description / group of a type.

use crate::entry::Entry;
use std::path::Path;

pub const DIRECTORY: &str = "inode/directory";
pub const UNKNOWN: &str = "application/octet-stream";

/// MIME type from the name only.
pub fn mime_from_name(name: &str) -> Option<String> {
    mime_guess::from_path(name).first().map(|m| m.essence_str().to_string())
}

/// MIME type of `path`: directories, then extension, then content sniffing of a regular file.
pub fn detect(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return DIRECTORY.to_string();
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if let Some(m) = mime_from_name(name) {
            return m;
        }
    }
    match tree_magic_mini::from_filepath(path) {
        Some(m) if m != UNKNOWN => m.to_string(),
        _ => UNKNOWN.to_string(),
    }
}

/// Cached MIME of an entry (extension based; call [`detect`] to sniff).
pub fn entry_mime(e: &Entry) -> String {
    if let Some(m) = &e.mime {
        return m.clone();
    }
    if e.is_dir {
        return DIRECTORY.to_string();
    }
    mime_from_name(&e.name).unwrap_or_else(|| UNKNOWN.to_string())
}

/// Groups of the "arrange by kind" view. The mapping of types to groups mirrors the table shipped
/// with the design; the display order of the groups is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KindGroup {
    Applications,
    Folders,
    Documents,
    Images,
    Movies,
    Music,
    Pdf,
    Text,
    Spreadsheets,
    Presentations,
    Email,
    Fonts,
    Source,
    Contacts,
    SystemPrefs,
    Other,
}

impl KindGroup {
    pub fn title(self) -> &'static str {
        match self {
            KindGroup::Applications => "Applications",
            KindGroup::Folders => "Folders",
            KindGroup::Documents => "Documents",
            KindGroup::Images => "Images",
            KindGroup::Movies => "Movies",
            KindGroup::Music => "Music",
            KindGroup::Pdf => "PDF Documents",
            KindGroup::Text => "Text",
            KindGroup::Spreadsheets => "Spreadsheets",
            KindGroup::Presentations => "Presentations",
            KindGroup::Email => "Mail",
            KindGroup::Fonts => "Fonts",
            KindGroup::Source => "Source Code",
            KindGroup::Contacts => "Contacts",
            KindGroup::SystemPrefs => "System Preferences",
            KindGroup::Other => "Other",
        }
    }
}

/// Group of a MIME type, following the structure of the design table (most specific first).
pub fn kind_group(mime: &str) -> KindGroup {
    match mime {
        DIRECTORY => KindGroup::Folders,
        "application/pdf" => KindGroup::Pdf,
        "application/x-executable" | "application/x-sharedlib" | "application/vnd.appimage" | "application/x-desktop" => KindGroup::Applications,
        "application/vnd.ms-excel" | "text/csv" | "application/vnd.oasis.opendocument.spreadsheet" => KindGroup::Spreadsheets,
        "application/vnd.ms-powerpoint" | "application/vnd.oasis.opendocument.presentation" => KindGroup::Presentations,
        "message/rfc822" | "application/mbox" => KindGroup::Email,
        "text/vcard" | "text/x-vcard" => KindGroup::Contacts,
        "text/html" | "text/x-csrc" | "text/x-c++src" | "text/x-rust" | "text/x-python" | "application/javascript" | "text/javascript" | "application/json" | "application/xml" | "text/x-java" | "text/x-shellscript" => KindGroup::Source,
        "application/rtf" | "text/rtf" | "text/plain" | "application/msword" | "application/vnd.oasis.opendocument.text" => KindGroup::Text,
        m if m.starts_with("font/") || m.starts_with("application/font") || m == "application/x-font-ttf" => KindGroup::Fonts,
        m if m.starts_with("image/") => KindGroup::Images,
        m if m.starts_with("video/") => KindGroup::Movies,
        m if m.starts_with("audio/") => KindGroup::Music,
        m if m.starts_with("text/") => KindGroup::Text,
        _ => KindGroup::Documents,
    }
}

/// The design's kind strings by MIME type: the descriptions of its type declarations (strings),
/// each for the MIME type of the declaration it describes (correspondence). A desktop entry is described as
/// the application it launches, and plain text by its parent text type, the plain-text declaration having
/// no description.
const KIND_STRINGS: [(&str, &str); 27] = [
    (DIRECTORY, "folder"),
    ("application/x-desktop", "application"),
    ("text/plain", "text"),
    ("image/png", "PNG image"),
    ("image/jpeg", "JPEG image"),
    ("application/pdf", "PDF document"),
    ("application/zip", "Zip archive"),
    ("application/gzip", "GZip archive"),
    ("application/x-tar", "tar archive"),
    ("video/mp4", "MPEG-4 movie"),
    ("audio/mp4", "MPEG-4 audio"),
    ("audio/mpeg", "MP3 audio"),
    ("video/quicktime", "QuickTime movie"),
    ("text/html", "HTML text"),
    ("application/xml", "XML text"),
    ("text/xml", "XML text"),
    ("application/json", "JSON"),
    ("text/x-python", "Python script"),
    ("text/x-python3", "Python script"),
    ("application/x-shellscript", "shell script"),
    ("text/x-csrc", "C source code"),
    ("application/x-executable", "Unix executable"),
    ("inode/symlink", "symbolic link"),
    ("image/svg+xml", "SVG image"),
    ("image/tiff", "TIFF image"),
    ("application/rtf", "rich text (RTF)"),
    ("text/rtf", "rich text (RTF)"),
];

/// Short human readable description used by the Kind column and by sorting by kind: the design's string
/// ([`KIND_STRINGS`]) with its first letter capitalised, as the design shows it; types it does
/// not list are described from the MIME type in the same style (approximate).
pub fn kind_description(mime: &str) -> String {
    let described = match KIND_STRINGS.iter().find(|(m, _)| *m == mime) {
        Some((_, kind)) => (*kind).to_string(),
        None => match mime {
            m if m.contains("spreadsheet") || m == "text/csv" => "spreadsheet".into(),
            m if m.contains("wordprocessing") || m == "application/msword" => "word processing document".into(),
            m if m.contains("presentation") => "presentation".into(),
            "text/x-rust" => "Rust source code".into(),
            // a file of no known type: a generic document (approximate, the earlier behaviour; the design's description of
            // an undeclared type is not known)
            UNKNOWN => "document".into(),
            m => {
                let (major, minor) = m.split_once('/').unwrap_or((m, ""));
                let minor = minor.trim_start_matches("x-").trim_start_matches("vnd.").to_uppercase();
                match major {
                    "image" => format!("{minor} image"),
                    "video" => format!("{minor} movie"),
                    "audio" => format!("{minor} audio"),
                    "text" => format!("{minor} text"),
                    _ => format!("{minor} document"),
                }
            }
        },
    };
    let mut chars = described.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => described,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_detection_and_groups() {
        assert_eq!(mime_from_name("photo.png").as_deref(), Some("image/png"));
        assert_eq!(kind_group("image/png"), KindGroup::Images);
        assert_eq!(kind_group("video/mp4"), KindGroup::Movies);
        assert_eq!(kind_group("audio/mpeg"), KindGroup::Music);
        assert_eq!(kind_group("application/pdf"), KindGroup::Pdf);
        assert_eq!(kind_group(DIRECTORY), KindGroup::Folders);
        assert_eq!(kind_group("application/zip"), KindGroup::Documents);
    }

    #[test]
    fn sniffing_without_extension() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob");
        std::fs::write(&p, b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
        assert_eq!(detect(&p, false), "image/png");
        assert_eq!(detect(dir.path(), true), DIRECTORY);
    }

    #[test]
    fn descriptions() {
        assert_eq!(kind_description(DIRECTORY), "Folder");
        assert_eq!(kind_description("application/x-desktop"), "Application");
        assert_eq!(kind_description("image/png"), "PNG image");
        assert_eq!(kind_description("application/x-tar"), "Tar archive");
        assert_eq!(kind_description("application/x-shellscript"), "Shell script");
        assert_eq!(kind_description("application/rtf"), "Rich text (RTF)");
        assert_eq!(kind_description("text/plain"), "Text");
        assert_eq!(kind_description(UNKNOWN), "Document");
        assert_eq!(kind_description("image/webp"), "WEBP image", "an unlisted type in the same style");
    }
}
