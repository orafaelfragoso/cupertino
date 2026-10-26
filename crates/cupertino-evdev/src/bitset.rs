//! Kernel-layout bitmaps (`EVIOCGBIT`, `EVIOCGKEY`, ...): bit `n` is bit `n % 8` of byte `n / 8`.

/// A growable set of small integers in the kernel's bitmap layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitSet(Vec<u8>);

impl BitSet {
    /// Wraps a bitmap exactly as an ioctl returned it.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// A set holding exactly `codes`.
    pub fn from_codes(codes: &[u16]) -> Self {
        let mut set = Self(Vec::new());
        for &code in codes {
            set.insert(code, true);
        }
        set
    }

    /// Whether `bit` is set; bits past the end are clear.
    pub fn contains(&self, bit: u16) -> bool {
        self.0.get(usize::from(bit) / 8).is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
    }

    /// Sets or clears `bit`, growing the bitmap when needed.
    pub fn insert(&mut self, bit: u16, value: bool) {
        let index = usize::from(bit) / 8;
        if index >= self.0.len() {
            self.0.resize(index + 1, 0);
        }
        let mask = 1u8 << (bit % 8);
        if value {
            self.0[index] |= mask;
        } else {
            self.0[index] &= !mask;
        }
    }

    /// The set bits in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        (0..self.0.len() * 8).filter_map(|bit| {
            let bit = u16::try_from(bit).ok()?;
            self.contains(bit).then_some(bit)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_bit_layout() {
        // KEY_A (30) is bit 6 of byte 3.
        let set = BitSet::from_bytes(vec![0, 0, 0, 0b0100_0000]);
        assert!(set.contains(30));
        assert!(!set.contains(31));
        assert!(!set.contains(4000));
        assert_eq!(set.iter().collect::<Vec<_>>(), vec![30]);
    }

    #[test]
    fn insert_grows_and_clears() {
        let mut set = BitSet::default();
        set.insert(0x110, true);
        assert!(set.contains(0x110));
        set.insert(0x110, false);
        assert!(!set.contains(0x110));
    }
}
