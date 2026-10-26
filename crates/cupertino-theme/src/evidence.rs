#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Evidence {
    Unknown,
    Approximated,
    Inferred,
    Documented,
    Verified,
}

impl Evidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Evidence::Verified => "VERIFIED",
            Evidence::Documented => "DOCUMENTED",
            Evidence::Inferred => "INFERRED",
            Evidence::Approximated => "APPROXIMATED",
            Evidence::Unknown => "UNKNOWN",
        }
    }

    /// Weakest class mentioned in a free-form label such as `" (formula) / (unit)".
    /// Text without any class parses as [`:Unknown`].
    pub fn parse_weakest(s: &str) -> Evidence {
        const ALL: [Evidence; 5] = [
            Evidence::Verified,
            Evidence::Documented,
            Evidence::Inferred,
            Evidence::Approximated,
            Evidence::Unknown,
        ];
        ALL.iter()
            .copied()
            .filter(|e| s.contains(e.as_str()))
            .min()
            .unwrap_or(Evidence::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_weakest() {
        assert_eq!(Evidence::parse_weakest("VERIFIED"), Evidence::Verified);
        assert_eq!(
            Evidence::parse_weakest("VERIFIED (formula) / INFERRED (ms unit)"),
            Evidence::Inferred
        );
        assert_eq!(
            Evidence::parse_weakest("VERIFIED (value) / UNKNOWN (usage)"),
            Evidence::Unknown
        );
        assert_eq!(Evidence::parse_weakest("nonsense"), Evidence::Unknown);
        assert!(Evidence::Verified > Evidence::Approximated);
    }
}
