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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Model {
        next: u32,
        live: Vec<u32>,
    }

    impl TargetSource for Model {
        type Target = u32;
        fn create(&mut self, _: (u32, u32), _: TextureFormat) -> Result<u32> {
            self.next += 1;
            self.live.push(self.next);
            Ok(self.next)
        }
        fn destroy(&mut self, t: u32) {
            self.live.retain(|x| *x != t);
        }
    }

    #[test]
    fn a_target_costs_its_bytes_in_whole_granules() {
        let mib = 1 << 20;
        assert_eq!(cost((16, 16), TextureFormat::Rgba8), 2 * mib);
        assert_eq!(cost((1024, 512), TextureFormat::Rgba8), 2 * mib);
        assert_eq!(cost((1024, 513), TextureFormat::Rgba8), 4 * mib);
        assert_eq!(cost((3840, 2160), TextureFormat::Rgba8), 32 * mib);
        assert_eq!(TARGET_POOL_CAP, 16 * mib);
    }

    #[test]
    fn a_released_target_is_handed_out_again_for_its_size_and_format_only() {
        let (mut pool, mut src) = (TargetPool::new(TARGET_POOL_CAP), Model::default());
        let a = pool
            .acquire(&mut src, (64, 64), TextureFormat::Rgba8)
            .unwrap();
        let id = a.target;
        pool.release(a, 0);
        let b = pool
            .acquire(&mut src, (64, 64), TextureFormat::Rgba8)
            .unwrap();
        assert_eq!(b.target, id);
        assert_eq!(pool.made(), 1);
        let c = pool
            .acquire(&mut src, (64, 64), TextureFormat::Rgba16Float)
            .unwrap();
        assert_ne!(c.target, id);
        assert_eq!(pool.made(), 2);
        assert_eq!(pool.bytes(), (4 << 20, 0));
        pool.release(b, 1);
        pool.release(c, 1);
        pool.trim(&mut src);
        assert!(src.live.is_empty());
    }

    #[test]
    fn the_cap_frees_the_oldest_free_targets_and_refuses_what_targets_in_use_cannot_leave_room_for()
    {
        let (mut pool, mut src) = (TargetPool::new(8 << 20), Model::default());
        // four 2 MiB targets fill the cap
        let t: Vec<_> = (0..4)
            .map(|i| {
                pool.acquire(&mut src, (100 + i, 100), TextureFormat::Rgba8)
                    .unwrap()
            })
            .collect();
        let mut t = t.into_iter();
        let (a, b) = (t.next().unwrap(), t.next().unwrap());
        let (a_id, b_id) = (a.target, b.target);
        pool.release(a, 1);
        pool.release(b, 2);
        // a new size frees the free target used least recently, a
        let _e = pool
            .acquire(&mut src, (300, 100), TextureFormat::Rgba8)
            .unwrap();
        assert!(!src.live.contains(&a_id) && src.live.contains(&b_id));
        // 6 MiB in use and b free: 4 MiB more frees b and still does not fit
        match pool.acquire(&mut src, (1024, 1024), TextureFormat::Rgba8) {
            Err(Error::OverBudget { need, cap, .. }) => assert_eq!((need, cap), (4 << 20, 8 << 20)),
            other => panic!("{other:?}"),
        }
        assert!(!src.live.contains(&b_id));
    }
}
