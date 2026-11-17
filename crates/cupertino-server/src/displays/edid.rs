//! The identity a display reports in its EDID base block (VESA E-EDID 1.4): the maker's PNP ID, the
//! product name or code and the serial number. Used to name a head and to key its stored settings.

/// What a display says it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EdidIdentity {
    /// Three-letter PNP manufacturer ID, for example `GSM`.
    pub make: String,
    /// The display product name descriptor, else the product code as `0x` and four hex digits.
    pub model: String,
    /// The serial number descriptor, else the numeric serial as `0x` and eight hex digits; `None`
    /// when the display reports neither.
    pub serial: Option<String>,
}

const HEADER: [u8; 8] = [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00];
const BASE_BLOCK: usize = 128;
const DESCRIPTORS: [usize; 4] = [54, 72, 90, 108];
const TAG_SERIAL: u8 = 0xff;
const TAG_NAME: u8 = 0xfc;

/// Parse the base block. `None` when `edid` is not an EDID (too short, wrong header or bad checksum).
pub fn parse(edid: &[u8]) -> Option<EdidIdentity> {
    let block = edid.get(..BASE_BLOCK)?;
    if block[..8] != HEADER || block.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) != 0 {
        return None;
    }
    let id = u16::from_be_bytes([block[8], block[9]]);
    let letter = |shift: u16| char::from(b'A' - 1 + ((id >> shift) & 0x1f) as u8);
    let make: String = [letter(10), letter(5), letter(0)].into_iter().collect();
    let product = u16::from_le_bytes([block[10], block[11]]);
    let number = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);
    let text = |tag: u8| {
        DESCRIPTORS
            .iter()
            .map(|&at| &block[at..at + 18])
            .find(|d| d[..3] == [0, 0, 0] && d[3] == tag)
            .map(|d| {
                let raw = &d[5..18];
                let end = raw.iter().position(|&b| b == b'\n').unwrap_or(raw.len());
                String::from_utf8_lossy(&raw[..end]).trim().to_string()
            })
    };
    let model = text(TAG_NAME)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("0x{product:04x}"));
    let serial = text(TAG_SERIAL)
        .filter(|s| !s.is_empty())
        .or_else(|| (number != 0).then(|| format!("0x{number:08x}")));
    Some(EdidIdentity {
        make,
        model,
        serial,
    })
}
