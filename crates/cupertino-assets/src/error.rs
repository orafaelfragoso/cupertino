use std::fmt;
use std::path::PathBuf;

/// Why an asset pack, or a font of it, could not be used.
#[derive(Debug)]
pub enum AssetError {
    /// A file or directory could not be read.
    Read { path: PathBuf, source: std::io::Error },
    /// The manifest is malformed, has an unsupported version, or names something that is not there.
    Manifest { path: PathBuf, reason: String },
    /// The fonts of the directory cannot serve a requested role (raised by the text renderers).
    Font { reason: String },
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {} ({source})", path.display()),
            Self::Manifest { path, reason } => write!(f, "invalid asset manifest {}: {reason}", path.display()),
            Self::Font { reason } => f.write_str(reason),
        }
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}
