use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    Io { path: PathBuf, source: std::io::Error },
    Json(serde_json::Error),
    NotFound(String),
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Error::Json(e) => write!(f, "invalid specification JSON: {e}"),
            Error::NotFound(what) => write!(f, "not found: {what}"),
            Error::Invalid(what) => write!(f, "invalid specification data: {what}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}
