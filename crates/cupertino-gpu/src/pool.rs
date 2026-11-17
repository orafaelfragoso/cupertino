//! Pooled render targets: intermediate targets (blur levels, layers being composed) kept after use and handed out
//! again for the same size and format, so a frame does not allocate.
//!
//! The pool counts what its targets cost the driver, each rounded up to [`ALLOCATION_GRANULE`], and keeps the total
//! (in use and free) under its cap: a target that would pass it first frees the free targets used least recently,
//! and fails with [`Error::OverBudget`] when the targets in use alone would pass it. [`TARGET_POOL_CAP`] is half the
//! 32 MiB the design gives a GPU window for atlases and pools; the atlases have the other half. A window's own
//! buffers and its window-sized layer are outside the pool. The first render target of a process makes the driver
//! reserve one more target's size beside it, which no cap here can hold; the budget names it separately.

use crate::gles::{Device, Texture};
use crate::types::{Error, Result, TextureDesc, TextureFormat, TextureUsage};

/// The memory the driver rounds a render target up to.
pub const ALLOCATION_GRANULE: u64 = 2 << 20;
/// Most bytes of pooled targets, in use and free together.
pub const TARGET_POOL_CAP: u64 = 16 << 20;

/// What a target of `size` and `format` costs the driver.
pub fn cost(size: (u32, u32), format: TextureFormat) -> u64 {
    let bytes = u64::from(size.0) * u64::from(size.1) * u64::from(format.bytes_per_pixel());
    bytes.div_ceil(ALLOCATION_GRANULE) * ALLOCATION_GRANULE
}

/// What makes and frees the pool's targets: a device, or a model in tests.
pub trait TargetSource {
    type Target;
    fn create(&mut self, size: (u32, u32), format: TextureFormat) -> Result<Self::Target>;
    fn destroy(&mut self, target: Self::Target);
}

impl TargetSource for &Device {
    type Target = Texture;

    fn create(&mut self, size: (u32, u32), format: TextureFormat) -> Result<Texture> {
        self.create_texture(&TextureDesc {
            size,
            format,
            usage: TextureUsage::TARGET,
        })
    }

    fn destroy(&mut self, target: Texture) {
        self.destroy_texture(target)
    }
}

/// A target handed out by the pool; give it back with [`TargetPool::release`].
#[derive(Debug)]
pub struct Pooled<T> {
    pub target: T,
    pub size: (u32, u32),
    pub format: TextureFormat,
}

#[derive(Debug)]
struct Free<T> {
    target: Pooled<T>,
    last_use: u64,
}

/// A pool of render targets with a cap.
#[derive(Debug)]
pub struct TargetPool<T> {
    cap: u64,
    free: Vec<Free<T>>,
    in_use: u64,
    free_bytes: u64,
    made: u64,
}

impl<T> TargetPool<T> {
    pub fn new(cap: u64) -> Self {
        Self {
            cap,
            free: Vec::new(),
            in_use: 0,
            free_bytes: 0,
            made: 0,
        }
    }

    /// Bytes of targets in use and free.
    pub fn bytes(&self) -> (u64, u64) {
        (self.in_use, self.free_bytes)
    }

    /// Targets made in all, not taken from the free ones.
    pub fn made(&self) -> u64 {
        self.made
    }

    /// A target of `size` and `format`: a free one of that size and format, or a new one.
    pub fn acquire<S: TargetSource<Target = T>>(
        &mut self,
        source: &mut S,
        size: (u32, u32),
        format: TextureFormat,
    ) -> Result<Pooled<T>> {
        let c = cost(size, format);
        if let Some(i) = self
            .free
            .iter()
            .position(|f| f.target.size == size && f.target.format == format)
        {
            let f = self.free.swap_remove(i);
            self.free_bytes -= c;
            self.in_use += c;
            return Ok(f.target);
        }
        while self.in_use + self.free_bytes + c > self.cap && !self.free.is_empty() {
            let oldest = self
                .free
                .iter()
                .enumerate()
                .min_by_key(|(_, f)| f.last_use)
                .map(|(i, _)| i)
                .expect("not empty");
            let f = self.free.swap_remove(oldest);
            self.free_bytes -= cost(f.target.size, f.target.format);
            source.destroy(f.target.target);
        }
        if self.in_use + c > self.cap {
            return Err(Error::OverBudget {
                what: "render target pool",
                need: c,
                cap: self.cap,
            });
        }
        let target = source.create(size, format)?;
        self.made += 1;
        self.in_use += c;
        Ok(Pooled {
            target,
            size,
            format,
        })
    }

    /// Give a target back, last used in `frame`.
    pub fn release(&mut self, target: Pooled<T>, frame: u64) {
        let c = cost(target.size, target.format);
        self.in_use -= c.min(self.in_use);
        self.free_bytes += c;
        self.free.push(Free {
            target,
            last_use: frame,
        });
    }

    /// Free every target not in use (a hidden window drops its pools).
    pub fn trim<S: TargetSource<Target = T>>(&mut self, source: &mut S) {
        for f in self.free.drain(..) {
            source.destroy(f.target.target);
        }
        self.free_bytes = 0;
    }
}
