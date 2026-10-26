use std::fmt;

/// A 48-bit hardware address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacAddr(pub [u8; 6]);

impl fmt::Display for MacAddr {
    /// Lower-case hex pairs separated by colons.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(f, "{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{g:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_lower_case_pairs() {
        assert_eq!(
            MacAddr([0x02, 0x00, 0x0a, 0xbc, 0xde, 0xf0]).to_string(),
            "02:00:0a:bc:de:f0"
        );
    }
}
