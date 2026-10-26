//! Minimal reader for binary property lists (`bplist00`), enough for the per-cursor metadata
//! files: dictionaries, arrays, strings, integers, reals, booleans. Other object kinds (dates,
//! data) are parsed to [`Value::Other`]. All offsets are bounds-checked and nesting is
//! limited, so malformed input returns an error instead of panicking.

use crate::meta::RawMeta;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Real(f64),
    Str(String),
    Array(Vec<Value>),
    Dict(BTreeMap<String, Value>),
    Other,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            _ => None,
        }
    }
}

const MAX_DEPTH: usize = 32;

struct Reader<'a> {
    b: &'a [u8],
    offsets: Vec<usize>,
    ref_size: usize,
}

fn be(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |a, &x| (a << 8) | x as u64)
}

impl Reader<'_> {
    fn slice(&self, at: usize, len: usize) -> Result<&[u8], String> {
        at.checked_add(len).and_then(|e| self.b.get(at..e)).ok_or_else(|| "offset out of range".to_string())
    }

    fn int_at(&self, at: usize, size: usize) -> Result<u64, String> {
        Ok(be(self.slice(at, size)?))
    }

    /// Length of a counted object starting at `at` with marker nibble `n`; returns (length, data start).
    fn length(&self, at: usize, n: u8) -> Result<(usize, usize), String> {
        if n != 0xf {
            return Ok((n as usize, at + 1));
        }
        let m = *self.b.get(at + 1).ok_or("truncated length")?;
        if m >> 4 != 1 {
            return Err("bad length marker".into());
        }
        let size = 1usize << (m & 0xf);
        if size > 8 {
            return Err("length too large".into());
        }
        Ok((self.int_at(at + 2, size)? as usize, at + 2 + size))
    }

    fn object(&self, index: usize, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("nesting too deep".into());
        }
        let at = *self.offsets.get(index).ok_or("object index out of range")?;
        let marker = *self.b.get(at).ok_or("object offset out of range")?;
        let (hi, lo) = (marker >> 4, marker & 0xf);
        Ok(match hi {
            0x0 => match lo {
                0x8 => Value::Bool(false),
                0x9 => Value::Bool(true),
                _ => Value::Other,
            },
            0x1 => {
                let size = 1usize << lo;
                if size > 8 {
                    return Err("integer too large".into());
                }
                let raw = self.int_at(at + 1, size)?;
                Value::Int(raw as i64)
            }
            0x2 => match lo {
                2 => Value::Real(f32::from_bits(self.int_at(at + 1, 4)? as u32) as f64),
                3 => Value::Real(f64::from_bits(self.int_at(at + 1, 8)?)),
                _ => return Err("unsupported real size".into()),
            },
            0x5 => {
                let (len, start) = self.length(at, lo)?;
                Value::Str(String::from_utf8_lossy(self.slice(start, len)?).into_owned())
            }
            0x6 => {
                let (len, start) = self.length(at, lo)?;
                let raw = self.slice(start, len.checked_mul(2).ok_or("length overflow")?)?;
                let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                Value::Str(String::from_utf16_lossy(&units))
            }
            0xa => {
                let (len, start) = self.length(at, lo)?;
                let mut v = Vec::new();
                for i in 0..len {
                    let r = self.int_at(start + i * self.ref_size, self.ref_size)? as usize;
                    v.push(self.object(r, depth + 1)?);
                }
                Value::Array(v)
            }
            0xd => {
                let (len, start) = self.length(at, lo)?;
                let mut map = BTreeMap::new();
                for i in 0..len {
                    let k = self.int_at(start + i * self.ref_size, self.ref_size)? as usize;
                    let v = self.int_at(start + (len + i) * self.ref_size, self.ref_size)? as usize;
                    let Value::Str(key) = self.object(k, depth + 1)? else { return Err("dictionary key is not a string".into()) };
                    map.insert(key, self.object(v, depth + 1)?);
                }
                Value::Dict(map)
            }
            _ => Value::Other,
        })
    }
}

/// Parse a binary property list and return its root object.
pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() < 8 + 32 || &bytes[..8] != b"bplist00" {
        return Err("not a binary property list".into());
    }
    let t = &bytes[bytes.len() - 32..];
    let (offset_size, ref_size) = (t[6] as usize, t[7] as usize);
    let (count, top, table) = (be(&t[8..16]) as usize, be(&t[16..24]) as usize, be(&t[24..32]) as usize);
    if !(1..=8).contains(&offset_size) || !(1..=8).contains(&ref_size) || count == 0 || count > 1_000_000 {
        return Err("invalid trailer".into());
    }
    let mut r = Reader { b: bytes, offsets: Vec::with_capacity(count), ref_size };
    for i in 0..count {
        let at = table.checked_add(i.checked_mul(offset_size).ok_or("overflow")?).ok_or("overflow")?;
        let off = r.int_at(at, offset_size)? as usize;
        r.offsets.push(off);
    }
    r.object(top, 0)
}

/// Convert the root dictionary of a cursor's property list to metadata keys.
pub fn raw_meta(root: &Value) -> Result<RawMeta, String> {
    let Value::Dict(d) = root else { return Err("the property list root is not a dictionary".into()) };
    let num = |k: &str| d.get(k).and_then(Value::as_f64);
    let flag = |k: &str| match d.get(k) {
        Some(Value::Bool(b)) => Some(*b),
        Some(v) => v.as_f64().map(|x| x != 0.0),
        None => None,
    };
    let color = match d.get("shadowcolor") {
        Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_f64).collect::<Vec<_>>()),
        _ => None,
    };
    Ok(RawMeta {
        hotx: num("hotx"),
        hoty: num("hoty"),
        hotx_scaled: num("hotx-scaled"),
        hoty_scaled: num("hoty-scaled"),
        shadowcolor: color,
        shadowoffsetx: num("shadowoffsetx"),
        shadowoffsety: num("shadowoffsety"),
        blur: num("blur"),
        frames: num("frames"),
        delay: num("delay"),
        vectoronly: flag("vectoronly"),
        rendershadow: flag("rendershadow"),
    })
}

pub fn raw_meta_from_bytes(bytes: &[u8]) -> Result<RawMeta, String> {
    raw_meta(&parse(bytes)?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn count(n: usize, hi: u8) -> Vec<u8> {
        if n < 15 {
            vec![(hi << 4) | n as u8]
        } else {
            let mut b = vec![(hi << 4) | 0xf, 0x12];
            b.extend_from_slice(&(n as u32).to_be_bytes());
            b
        }
    }

    /// Object bytes and child references of `v`; children are appended to `entries`.
    fn emit(v: &Value, entries: &mut Vec<(Vec<u8>, Vec<usize>)>) -> usize {
        let id = entries.len();
        entries.push((Vec::new(), Vec::new()));
        let (bytes, refs): (Vec<u8>, Vec<usize>) = match v {
            Value::Bool(b) => (vec![if *b { 0x09 } else { 0x08 }], vec![]),
            Value::Int(i) => {
                let mut b = vec![0x13];
                b.extend_from_slice(&i.to_be_bytes());
                (b, vec![])
            }
            Value::Real(r) => {
                let mut b = vec![0x23];
                b.extend_from_slice(&r.to_bits().to_be_bytes());
                (b, vec![])
            }
            Value::Str(s) if s.is_ascii() => {
                let mut b = count(s.len(), 5);
                b.extend_from_slice(s.as_bytes());
                (b, vec![])
            }
            Value::Str(s) => {
                let units: Vec<u16> = s.encode_utf16().collect();
                let mut b = count(units.len(), 6);
                units.iter().for_each(|u| b.extend_from_slice(&u.to_be_bytes()));
                (b, vec![])
            }
            Value::Array(a) => {
                let refs: Vec<usize> = a.iter().map(|x| emit(x, entries)).collect();
                (count(a.len(), 0xa), refs)
            }
            Value::Dict(d) => {
                let keys: Vec<usize> = d.keys().map(|k| emit(&Value::Str(k.clone()), entries)).collect();
                let vals: Vec<usize> = d.values().map(|x| emit(x, entries)).collect();
                (count(d.len(), 0xd), keys.into_iter().chain(vals).collect())
            }
            Value::Other => (vec![0x00], vec![]),
        };
        entries[id] = (bytes, refs);
        id
    }

    /// Encode a value tree as a binary property list (test helper: ints use 8 bytes, reals 8,
    /// two-byte references, four-byte offsets).
    pub fn encode(root: &Value) -> Vec<u8> {
        let mut entries: Vec<(Vec<u8>, Vec<usize>)> = Vec::new();
        emit(root, &mut entries);
        let mut out = b"bplist00".to_vec();
        let mut offsets = Vec::new();
        for (bytes, refs) in &entries {
            offsets.push(out.len());
            out.extend_from_slice(bytes);
            for r in refs {
                out.extend_from_slice(&(*r as u16).to_be_bytes());
            }
        }
        let table = out.len();
        for o in &offsets {
            out.extend_from_slice(&(*o as u32).to_be_bytes());
        }
        let mut trailer = vec![0u8; 5];
        trailer.extend_from_slice(&[0, 4, 2]);
        trailer.extend_from_slice(&(entries.len() as u64).to_be_bytes());
        trailer.extend_from_slice(&0u64.to_be_bytes());
        trailer.extend_from_slice(&(table as u64).to_be_bytes());
        out.extend_from_slice(&trailer);
        out
    }

    fn dict(items: &[(&str, Value)]) -> Value {
        Value::Dict(items.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    #[test]
    fn round_trips_scalars_arrays_and_dictionaries() {
        let v = dict(&[
            ("hotx", Value::Int(5)),
            ("blur", Value::Real(1.8)),
            ("vectoronly", Value::Bool(true)),
            ("shadowcolor", Value::Array(vec![Value::Real(0.0), Value::Real(0.0), Value::Real(0.0), Value::Real(0.65)])),
            ("name", Value::Str("héllo".into())),
            ("long", Value::Str("x".repeat(40))),
        ]);
        assert_eq!(parse(&encode(&v)).unwrap(), v);
    }

    #[test]
    fn converts_to_cursor_metadata() {
        let v = dict(&[
            ("hotx", Value::Int(12)),
            ("hoty", Value::Int(9)),
            ("hotx-scaled", Value::Real(13.0)),
            ("hoty-scaled", Value::Int(10)),
            ("shadowcolor", Value::Array(vec![Value::Int(0), Value::Int(0), Value::Int(0), Value::Real(0.5)])),
            ("shadowoffsetx", Value::Int(0)),
            ("shadowoffsety", Value::Int(-1)),
            ("blur", Value::Real(0.8)),
            ("frames", Value::Int(6)),
            ("delay", Value::Real(0.5)),
            ("rendershadow", Value::Bool(true)),
        ]);
        let raw = raw_meta_from_bytes(&encode(&v)).unwrap();
        assert_eq!(raw.hotx, Some(12.0));
        assert_eq!(raw.hoty_scaled, Some(10.0));
        assert_eq!(raw.shadowcolor, Some(vec![0.0, 0.0, 0.0, 0.5]));
        assert_eq!(raw.shadowoffsety, Some(-1.0));
        assert_eq!(raw.frames, Some(6.0));
        assert_eq!(raw.rendershadow, Some(true));
        let meta = crate::meta::CursorMeta::from_raw(&raw, true);
        assert_eq!(meta.shadow.unwrap().dy, 1.0);
        assert_eq!(meta.delay_ms, 500);
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(parse(b"").is_err());
        assert!(parse(b"bplist00").is_err());
        assert!(parse(&[0u8; 100]).is_err());
        let good = encode(&dict(&[("a", Value::Int(1))]));
        for cut in [10, good.len() - 33, good.len() - 1] {
            let _ = parse(&good[..cut]);
        }
        let mut bad_table = good.clone();
        let n = bad_table.len();
        bad_table[n - 8] = 0xff;
        assert!(parse(&bad_table).is_err());
        let mut bad_count = good.clone();
        bad_count[n - 24] = 0x7f;
        assert!(parse(&bad_count).is_err());
        assert!(raw_meta(&Value::Int(1)).is_err());
    }

    #[test]
    fn cyclic_references_are_bounded() {
        let mut b = b"bplist00".to_vec();
        let off = b.len();
        b.extend_from_slice(&[0xa1, 0x00]);
        let table = b.len();
        b.extend_from_slice(&(off as u32).to_be_bytes());
        let mut trailer = vec![0u8; 5];
        trailer.extend_from_slice(&[0, 4, 1]);
        trailer.extend_from_slice(&1u64.to_be_bytes());
        trailer.extend_from_slice(&0u64.to_be_bytes());
        trailer.extend_from_slice(&(table as u64).to_be_bytes());
        b.extend_from_slice(&trailer);
        assert!(parse(&b).unwrap_err().contains("deep"));
    }

    #[test]
    fn real_reference_property_lists_parse_when_provided() {
        let Some(dir) = std::env::var_os("CUPERTINO_CURSOR_SOURCE") else { return };
        let mut n = 0;
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path().join("info.plist");
            if let Ok(bytes) = std::fs::read(&p) {
                if bytes.starts_with(b"bplist00") {
                    raw_meta_from_bytes(&bytes).unwrap_or_else(|err| panic!("{}: {err}", p.display()));
                    n += 1;
                }
            }
        }
        assert!(n > 0);
    }
}
