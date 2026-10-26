//! Table acceleration: piecewise-linear speed curves read from a binary acceleration table.
//!
//! the big-endian table format, the
//! setting → point-set construction (blend of the two bracketing entries, or `(a+1)/(acc+1)
//! scaling outside them, first point per x wins) and the segment evaluation from the origin with
//! extrapolation past the last point. A pointing device that ships neither curves nor a table of
//! its own gets [`DEFAULT_TABLE`] (for a generic mouse).

/// The design's embedded fallback table: scale 0.5, signature `@200`,
/// two entries: acceleration 0 → `(1, 1)`; acceleration 1 → nine points up to `(40, 150).
/// Laid out as header, then per entry its header followed by one row per point.
pub const DEFAULT_TABLE: [u8; 102] = [
    0x00, 0x00, 0x80, 0x00, 0x40, 0x32, 0x30, 0x30, 0x00, 0x02,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x09,
    0x00, 0x00, 0x71, 0x3b, 0x00, 0x00, 0x60, 0x00,
    0x00, 0x04, 0x4e, 0xc5, 0x00, 0x10, 0x80, 0x00,
    0x00, 0x0c, 0x00, 0x00, 0x00, 0x5f, 0x00, 0x00,
    0x00, 0x16, 0xec, 0x4f, 0x00, 0x8b, 0x00, 0x00,
    0x00, 0x1d, 0x3b, 0x14, 0x00, 0x94, 0x80, 0x00,
    0x00, 0x22, 0x76, 0x27, 0x00, 0x96, 0x00, 0x00,
    0x00, 0x24, 0x62, 0x76, 0x00, 0x96, 0x00, 0x00,
    0x00, 0x26, 0x00, 0x00, 0x00, 0x96, 0x00, 0x00,
    0x00, 0x28, 0x00, 0x00, 0x00, 0x96, 0x00, 0x00,
];

/// The two signatures a table may carry.
const SIGNATURES: [&[u8; 4]; 2] = [b"@200", b"USB*"];

/// One curve of a table: the setting it belongs to and its `(x, y)` points (inches per second
/// in and out, decoded from 16.16).
#[derive(Clone, Debug, PartialEq)]
pub struct TableEntry {
    pub acceleration: f64,
    pub points: Vec<(f64, f64)>,
}

/// A decoded acceleration table.
#[derive(Clone, Debug, PartialEq)]
pub struct AccelTable {
    /// Header scale (16.16). The design stores it but the curve construction never reads it.
    pub scale: f64,
    /// Entries in ascending `acceleration`, as stored.
    pub entries: Vec<TableEntry>,
}

/// Why a byte buffer is not a usable table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// The buffer ends before the last declared point. The design does no length check and
    /// reads past the buffer; here that is an error.
    Truncated,
    /// The signature is neither `@200` nor `USB* (the design no curve then).
    Signature([u8; 4]),
    /// An entry has no points, or a point at x ≤ 0: the curve segments start at the origin, so
    /// such a table has no defined slope.
    Degenerate,
}

/// Big-endian cursor over a table buffer.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], TableError> {
        let end = self.at + N;
        let chunk = self.bytes.get(self.at..end).ok_or(TableError::Truncated)?;
        self.at = end;
        Ok(chunk.try_into().expect("slice has length N"))
    }

    fn fixed(&mut self) -> Result<f64, TableError> {
        Ok(f64::from(i32::from_be_bytes(self.take()?)) / 65536.0)
    }

    fn count(&mut self) -> Result<u16, TableError> {
        Ok(u16::from_be_bytes(self.take()?))
    }
}

impl AccelTable {
    /// Parse the big-endian layout `{s32 scale, 4-byte signature, u16 count} followed by
    /// `count` entries `{s32 acceleration, u16 n, n × (s32 x, s32 y)}; every s32 is 16.16.
    pub fn parse(bytes: &[u8]) -> Result<Self, TableError> {
        let mut r = Reader { bytes, at: 0 };
        let scale = r.fixed()?;
        let signature: [u8; 4] = r.take()?;
        if !SIGNATURES.contains(&&signature) {
            return Err(TableError::Signature(signature));
        }
        let entries = (0..r.count()?)
            .map(|_| {
                let acceleration = r.fixed()?;
                let points: Vec<(f64, f64)> = (0..r.count()?).map(|_| Ok((r.fixed()?, r.fixed()?))).collect::<Result<_, _>>()?;
                if points.is_empty() || points.iter().any(|p| p.0 <= 0.0) {
                    return Err(TableError::Degenerate);
                }
                Ok(TableEntry { acceleration, points })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { scale, entries })
    }

    /// [`DEFAULT_TABLE`], decoded.
    pub fn default_table() -> Self {
        Self::parse(&DEFAULT_TABLE).expect("the embedded default table is well formed")
    }
}

/// Insert `(x, y)` unless a point with the same `x` exists: the design collects points in a
/// set ordered by x, where the first insert wins.
fn insert_first(points: &mut Vec<(f64, f64)>, x: f64, y: f64) {
    if !points.iter().any(|p| p.0 == x) {
        points.push((x, y));
    }
}

/// Blend every point of `from` toward the polyline of `toward` (which starts at the origin):
/// `y = min(P.y, line(P.x)) + |P.y − line(P.x)| · ratio`, where `line` extends the segment of
/// `toward` that covers `P.x` (the last one past its end)..
fn blend_into(from: &[(f64, f64)], toward: &[(f64, f64)], ratio: f64, out: &mut Vec<(f64, f64)>) {
    let mut j = 0;
    let mut prev = (0.0, 0.0);
    for &(px, py) in from {
        while px > toward[j].0 && j + 1 < toward.len() {
            prev = toward[j];
            j += 1;
        }
        let q = toward[j];
        let slope = (q.1 - prev.1) / (q.0 - prev.0);
        let line = slope * px + (q.1 - slope * q.0);
        insert_first(out, px, line.min(py) + (py - line).abs() * ratio);
    }
}

/// A table curve selected for one setting: piecewise linear from the origin through its points,
/// mapping input speed (inches per second) to output speed.
#[derive(Clone, Debug, PartialEq)]
pub struct TableCurve {
    /// Ascending x, unique.
    points: Vec<(f64, f64)>,
}

impl TableCurve {
    /// Build the curve for `setting`: the first entry with
    /// `acceleration ≥ setting` is `hi`, the one before it `lo`. An exact match, a setting below
    /// the first entry or above the last uses one entry with every y scaled by
    /// `(setting + 1) / (acceleration + 1); otherwise both entries are blended toward each other
    /// with `ratio = (setting − lo) / (hi − lo)`, `lo`'s points inserted first.
    ///
    /// `None` when the setting is negative (acceleration off) or the table is empty.
    pub fn new(table: &AccelTable, setting: f64) -> Option<Self> {
        if setting < 0.0 {
            return None;
        }
        let entries = &table.entries;
        let last = entries.len().checked_sub(1)?;
        let (lo, hi) = match entries.iter().position(|e| e.acceleration >= setting) {
            Some(i) if entries[i].acceleration == setting => (i, i),
            Some(i) => (i.saturating_sub(1), i),
            None => (last, last),
        };
        let mut points = Vec::new();
        if lo == hi {
            let e = &entries[hi];
            let k = (setting + 1.0) / (e.acceleration + 1.0);
            for &(x, y) in &e.points {
                insert_first(&mut points, x, y * k);
            }
        } else {
            let (lo, hi) = (&entries[lo], &entries[hi]);
            let ratio = (setting - lo.acceleration) / (hi.acceleration - lo.acceleration);
            blend_into(&lo.points, &hi.points, ratio, &mut points);
            blend_into(&hi.points, &lo.points, ratio, &mut points);
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        Some(Self { points })
    }

    /// Points of the curve, ascending x.
    pub fn points(&self) -> &[(f64, f64)] {
        &self.points
    }

    /// Output speed for input speed `v`: the segment ending at the first point with `x ≥ v`
    /// (the first segment starts at the origin); past the last point the last segment is
    /// extended.
    pub fn speed(&self, v: f64) -> f64 {
        let i = self.points.iter().position(|p| p.0 >= v).unwrap_or(self.points.len() - 1);
        let (x0, y0) = if i == 0 { (0.0, 0.0) } else { self.points[i - 1] };
        let (x1, y1) = self.points[i];
        y0 + (y1 - y0) / (x1 - x0) * (v - x0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_default_table() {
        let t = AccelTable::default_table();
        assert_eq!(t.scale, 0.5);
        assert_eq!(t.entries.len(), 2);
        assert_eq!(t.entries[0], TableEntry { acceleration: 0.0, points: vec![(1.0, 1.0)] });
        assert_eq!(t.entries[1].acceleration, 1.0);
        assert_eq!(
            t.entries[1].points,
            vec![
                (0.4423065185546875, 0.375),
                (4.3076934814453125, 16.5),
                (12.0, 95.0),
                (22.923080444335938, 139.0),
                (29.23077392578125, 148.5),
                (34.46153259277344, 150.0),
                (36.384613037109375, 150.0),
                (38.0, 150.0),
                (40.0, 150.0),
            ]
        );
    }

    #[test]
    fn rejects_short_buffers_and_unknown_signatures() {
        assert_eq!(AccelTable::parse(&DEFAULT_TABLE[..101]), Err(TableError::Truncated));
        // header + one entry {acceleration 0, n = 0}
        let empty = [0, 0, 0x80, 0, b'@', b'2', b'0', b'0', 0, 1, 0, 0, 0, 0, 0, 0];
        assert_eq!(AccelTable::parse(&empty), Err(TableError::Degenerate));
        // one entry with the point (0, 1)
        let origin = [0, 0, 0x80, 0, b'@', b'2', b'0', b'0', 0, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0, 0];
        assert_eq!(AccelTable::parse(&origin), Err(TableError::Degenerate));
        let mut bad = DEFAULT_TABLE;
        bad[4..8].copy_from_slice(b"ABCD");
        assert_eq!(AccelTable::parse(&bad), Err(TableError::Signature(*b"ABCD")));
    }

    #[test]
    fn blends_bracketing_entries_like_the_reference() {
        // Point set for setting 0.5.
        let c = TableCurve::new(&AccelTable::default_table(), 0.5).unwrap();
        let expected = [
            (0.4423065185546875, 0.40865325927734375),
            (1.0, 1.8507480104373095),
            (4.3076934814453125, 10.403846740722656),
            (12.0, 53.5),
            (22.923080444335938, 80.96154022216797),
            (29.23077392578125, 88.86538696289062),
            (34.46153259277344, 92.23076629638672),
            (36.384613037109375, 93.19230651855469),
            (38.0, 94.0),
            (40.0, 95.0),
        ];
        assert_eq!(c.points().len(), expected.len());
        for (got, want) in c.points().iter().zip(expected) {
            assert_eq!(got.0, want.0);
            assert!((got.1 - want.1).abs() < 1e-12, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn scales_outside_the_entries() {
        // Setting 3 is above the last entry (1.0): its y values double, (3+1)/(1+1).
        let c = TableCurve::new(&AccelTable::default_table(), 3.0).unwrap();
        assert_eq!(c.points()[0], (0.4423065185546875, 0.75));
        assert_eq!(c.points()[8], (40.0, 300.0));
        assert_eq!(TableCurve::new(&AccelTable::default_table(), -1.0), None);
    }

    #[test]
    fn extrapolates_past_the_last_point() {
        let c = TableCurve::new(&AccelTable::default_table(), 0.0).unwrap();
        assert_eq!(c.speed(0.5), 0.5);
        assert_eq!(c.speed(7.0), 7.0);
    }
}
