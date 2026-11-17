use std::fmt;

/// A kernel device number (`dev_t`): the major and minor of a device node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DevT(rustix::fs::Dev);

impl DevT {
    /// Combines a major and a minor number.
    pub fn new(major: u32, minor: u32) -> Self {
        Self(rustix::fs::makedev(major, minor))
    }

    /// Wraps a raw `dev_t` as returned by `stat`.
    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw `dev_t` value.
    pub fn as_raw(self) -> u64 {
        self.0
    }

    /// The major number.
    pub fn major(self) -> u32 {
        rustix::fs::major(self.0)
    }

    /// The minor number.
    pub fn minor(self) -> u32 {
        rustix::fs::minor(self.0)
    }
}

impl fmt::Display for DevT {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.major(), self.minor())
    }
}
