//! Windowing for long lists: only the rows near the viewport exist as nodes.

use crate::{ItemMetrics, LayoutError, VirtualWindow};

/// Row heights of a list and the arithmetic to find the rows in a viewport. Fixed heights take no storage and every
/// query is constant time; estimated heights keep a prefix-sum tree, so a measured row costs `O(log n)` to record and
/// to find, and 16 bytes per row.
#[derive(Clone, Debug)]
pub struct VirtualList {
    count: usize,
    spacing: f64,
    rows: Rows,
    /// The row at the top of the viewport at the last window, and where its top was.
    anchor: Option<(usize, f64)>,
}

#[derive(Clone, Debug)]
enum Rows {
    /// Row height plus spacing.
    Fixed(f64),
    Estimated {
        /// Estimated row height plus spacing; `heights` and `sums` hold row height plus spacing too.
        estimate: f64,
        heights: Vec<f64>,
        /// Fenwick tree over `heights`, 1-based.
        sums: Vec<f64>,
    },
}

impl VirtualList {
    /// `metrics` must hold a finite height above zero.
    pub fn new(count: usize, metrics: ItemMetrics) -> Result<Self, LayoutError> {
        Self::with_spacing(count, metrics, 0.0)
    }

    /// Like [`new`](Self::new), with `spacing` between neighbouring rows (not after the last). It must be finite and
    /// not negative.
    pub fn with_spacing(
        count: usize,
        metrics: ItemMetrics,
        spacing: f64,
    ) -> Result<Self, LayoutError> {
        if !spacing.is_finite() {
            return Err(LayoutError::NonFiniteValue);
        }
        if spacing < 0.0 {
            return Err(LayoutError::InvalidConstraint);
        }
        let height = match metrics {
            ItemMetrics::Fixed(h) | ItemMetrics::Estimated(h) => h,
        };
        if !height.is_finite() {
            return Err(LayoutError::NonFiniteValue);
        }
        if height <= 0.0 {
            return Err(LayoutError::InvalidConstraint);
        }
        let rows = match metrics {
            ItemMetrics::Fixed(h) => Rows::Fixed(h + spacing),
            ItemMetrics::Estimated(estimate) => {
                let stride = estimate + spacing;
                Rows::Estimated {
                    estimate: stride,
                    heights: vec![stride; count],
                    sums: build_sums(&vec![stride; count]),
                }
            }
        };
        Ok(Self {
            count,
            spacing,
            rows,
            anchor: None,
        })
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Changes the number of rows. Rows past the old end start at the estimate; measurements before it are kept.
    pub fn set_count(&mut self, count: usize) {
        if let Rows::Estimated {
            estimate,
            heights,
            sums,
        } = &mut self.rows
        {
            heights.resize(count, *estimate);
            *sums = build_sums(heights);
        }
        self.count = count;
        self.anchor = self.anchor.filter(|(index, _)| *index < count);
    }

    /// Records the measured height of a row. Ignored for fixed heights, out-of-range rows and heights that are not
    /// finite and non-negative.
    pub fn set_height(&mut self, index: usize, height: f64) {
        let Rows::Estimated { heights, sums, .. } = &mut self.rows else {
            return;
        };
        if !height.is_finite() || height < 0.0 {
            return;
        }
        let Some(slot) = heights.get_mut(index) else {
            return;
        };
        let stride = height + self.spacing;
        let delta = stride - *slot;
        *slot = stride;
        let mut at = index + 1;
        while let Some(sum) = sums.get_mut(at) {
            *sum += delta;
            at += at.isolate_lowest_one();
        }
    }

    /// The y of the top of a row; `index == count` gives where a further row would start.
    pub fn top(&self, index: usize) -> f64 {
        let index = index.min(self.count);
        match &self.rows {
            Rows::Fixed(h) => h * index as f64,
            Rows::Estimated { sums, .. } => {
                let mut total = 0.0;
                let mut at = index;
                while at > 0 {
                    total += sums.get(at).copied().unwrap_or(0.0);
                    at &= at - 1;
                }
                total
            }
        }
    }

    pub fn total_height(&self) -> f64 {
        match self.count {
            0 => 0.0,
            count => self.top(count) - self.spacing,
        }
    }

    /// The row containing `offset`; the last row for offsets past the end, `0` for an empty list.
    pub fn index_at(&self, offset: f64) -> usize {
        if self.count == 0 || offset.is_nan() || offset <= 0.0 {
            return 0;
        }
        match &self.rows {
            Rows::Fixed(h) => ((offset / h) as usize).min(self.count - 1),
            Rows::Estimated { sums, .. } => {
                let mut position = 0;
                let mut remaining = offset;
                let mut step = sums.len().saturating_sub(1).next_power_of_two();
                while step > 0 {
                    let next = position + step;
                    if let Some(sum) = sums.get(next).filter(|sum| **sum <= remaining) {
                        position = next;
                        remaining -= sum;
                    }
                    step >>= 1;
                }
                position.min(self.count - 1)
            }
        }
    }

    /// The rows to keep as nodes for a viewport `viewport` high at `offset`, plus `overscan` viewports on each side.
    /// `first..last` is half-open and empty for an empty list.
    ///
    /// When rows above the top row were measured since the last call, the top row moved. `anchor_adjust` is how far,
    /// and the window is computed at `offset + anchor_adjust`; the caller moves its scroll offset by it.
    pub fn window(&mut self, offset: f64, viewport: f64, overscan: f64) -> VirtualWindow {
        let offset = if offset.is_finite() {
            offset.max(0.0)
        } else {
            0.0
        };
        let viewport = if viewport.is_finite() {
            viewport.max(0.0)
        } else {
            0.0
        };
        let overscan = if overscan.is_finite() {
            overscan.max(0.0)
        } else {
            0.0
        };
        if self.count == 0 {
            self.anchor = None;
            return VirtualWindow {
                first: 0,
                last: 0,
                anchor_adjust: 0.0,
            };
        }
        let anchor_adjust = self
            .anchor
            .filter(|(index, _)| *index < self.count)
            .map_or(0.0, |(index, top)| self.top(index) - top);
        let offset = (offset + anchor_adjust).max(0.0);
        let visible = self.index_at(offset);
        self.anchor = Some((visible, self.top(visible)));
        let margin = viewport * overscan;
        let first = self.index_at(offset - margin);
        let last = (self.index_at(offset + viewport + margin) + 1).min(self.count);
        VirtualWindow {
            first,
            last,
            anchor_adjust,
        }
    }
}

fn build_sums(heights: &[f64]) -> Vec<f64> {
    let mut sums = vec![0.0; heights.len() + 1];
    for (i, height) in heights.iter().enumerate() {
        sums[i + 1] += height;
        let parent = (i + 1) + (i + 1).isolate_lowest_one();
        if parent < sums.len() {
            let carried = sums[i + 1];
            sums[parent] += carried;
        }
    }
    sums
}
