//! The pool of render targets, driven by a source that only counts.

use cupertino_gpu::pool::{cost, TargetPool, TargetSource, ALLOCATION_GRANULE, TARGET_POOL_CAP};
use cupertino_gpu::{Error, Result, TextureFormat};

const MIB: u64 = 1 << 20;

/// Hands out numbered targets and keeps a record of what was made and destroyed.
#[derive(Default)]
struct Counting {
    next: u32,
    created: Vec<(u32, (u32, u32), TextureFormat)>,
    destroyed: Vec<u32>,
    refuse: bool,
}

impl TargetSource for Counting {
    type Target = u32;

    fn create(&mut self, size: (u32, u32), format: TextureFormat) -> Result<u32> {
        if self.refuse {
            return Err(Error::Driver("no memory".to_owned()));
        }
        self.next += 1;
        self.created.push((self.next, size, format));
        Ok(self.next)
    }

    fn destroy(&mut self, target: u32) {
        self.destroyed.push(target);
    }
}

#[test]
fn a_target_costs_its_bytes_rounded_up_to_the_granule() {
    assert_eq!(ALLOCATION_GRANULE, 2 * MIB);
    assert_eq!(TARGET_POOL_CAP, 16 * MIB);
    assert_eq!(cost((1, 1), TextureFormat::R8), 2 * MIB);
    assert_eq!(cost((2048, 1024), TextureFormat::R8), 2 * MIB);
    assert_eq!(cost((2048, 1025), TextureFormat::R8), 4 * MIB);
    assert_eq!(cost((1024, 1024), TextureFormat::Rgba8), 4 * MIB);
    assert_eq!(cost((1024, 1024), TextureFormat::Rgba16Float), 8 * MIB);
    assert_eq!(cost((1024, 1024), TextureFormat::Rgba32Float), 16 * MIB);
    assert_eq!(cost((0, 100), TextureFormat::Rgba8), 0);
}

#[test]
fn a_new_pool_holds_nothing() {
    let pool = TargetPool::<u32>::new(TARGET_POOL_CAP);
    assert_eq!(pool.bytes(), (0, 0));
    assert_eq!(pool.made(), 0);
}

#[test]
fn acquiring_makes_a_target_and_counts_it_as_in_use() {
    let mut source = Counting::default();
    let mut pool = TargetPool::new(TARGET_POOL_CAP);
    let target = pool
        .acquire(&mut source, (1024, 1024), TextureFormat::Rgba8)
        .unwrap();
    assert_eq!(target.target, 1);
    assert_eq!(target.size, (1024, 1024));
    assert_eq!(target.format, TextureFormat::Rgba8);
    assert_eq!(pool.bytes(), (4 * MIB, 0));
    assert_eq!(pool.made(), 1);
}

#[test]
fn a_released_target_is_handed_out_again_for_the_same_size_and_format() {
    let mut source = Counting::default();
    let mut pool = TargetPool::new(TARGET_POOL_CAP);
    let target = pool
        .acquire(&mut source, (512, 512), TextureFormat::Rgba8)
        .unwrap();
    pool.release(target, 1);
    assert_eq!(pool.bytes(), (0, 2 * MIB));
    let again = pool
        .acquire(&mut source, (512, 512), TextureFormat::Rgba8)
        .unwrap();
    assert_eq!(again.target, 1);
    assert_eq!(pool.made(), 1);
    assert_eq!(pool.bytes(), (2 * MIB, 0));
    assert_eq!(source.created.len(), 1);
}

#[test]
fn another_size_or_format_makes_a_new_target() {
    let mut source = Counting::default();
    let mut pool = TargetPool::new(TARGET_POOL_CAP);
    let target = pool
        .acquire(&mut source, (512, 512), TextureFormat::Rgba8)
        .unwrap();
    pool.release(target, 1);
    let other_size = pool
        .acquire(&mut source, (512, 513), TextureFormat::Rgba8)
        .unwrap();
    let other_format = pool
        .acquire(&mut source, (512, 512), TextureFormat::Rgba16Float)
        .unwrap();
    assert_eq!((other_size.target, other_format.target), (2, 3));
    assert_eq!(pool.made(), 3);
}

#[test]
fn a_target_that_would_pass_the_cap_first_frees_the_free_target_used_least_recently() {
    let mut source = Counting::default();
    let mut pool = TargetPool::new(8 * MIB);
    let old = pool
        .acquire(&mut source, (1024, 1024), TextureFormat::Rgba8)
        .unwrap();
    let new = pool
        .acquire(&mut source, (1000, 1000), TextureFormat::Rgba8)
        .unwrap();
    // released out of order: `new` was last used in frame 1, `old` in frame 5
    pool.release(new, 1);
    pool.release(old, 5);
    assert_eq!(pool.bytes(), (0, 8 * MIB));
    pool.acquire(&mut source, (1024, 512), TextureFormat::Rgba8)
        .unwrap();
    assert_eq!(source.destroyed, [2], "the target of frame 1 goes first");
    assert_eq!(pool.bytes(), (2 * MIB, 4 * MIB));
}

#[test]
fn targets_in_use_are_never_freed_and_the_cap_refuses_instead() {
    let mut source = Counting::default();
    let mut pool = TargetPool::new(8 * MIB);
    let _a = pool
        .acquire(&mut source, (1024, 1024), TextureFormat::Rgba8)
        .unwrap();
    let _b = pool
        .acquire(&mut source, (1024, 1024), TextureFormat::Rgba8)
        .unwrap();
    match pool.acquire(&mut source, (64, 64), TextureFormat::Rgba8) {
        Err(Error::OverBudget { what, need, cap }) => {
            assert_eq!(what, "render target pool");
            assert_eq!(need, 2 * MIB);
            assert_eq!(cap, 8 * MIB);
        }
        other => panic!("{other:?}"),
    }
    assert!(source.destroyed.is_empty());
    assert_eq!(pool.made(), 2);
    assert_eq!(pool.bytes(), (8 * MIB, 0));
}

#[test]
fn a_refused_creation_leaves_the_books_as_they_were() {
    let mut source = Counting::default();
    let mut pool = TargetPool::<u32>::new(TARGET_POOL_CAP);
    source.refuse = true;
    assert!(matches!(
        pool.acquire(&mut source, (64, 64), TextureFormat::Rgba8),
        Err(Error::Driver(_))
    ));
    assert_eq!(pool.bytes(), (0, 0));
    assert_eq!(pool.made(), 0);
}

#[test]
fn trimming_frees_every_free_target_and_keeps_those_in_use() {
    let mut source = Counting::default();
    let mut pool = TargetPool::new(TARGET_POOL_CAP);
    let kept = pool
        .acquire(&mut source, (64, 64), TextureFormat::Rgba8)
        .unwrap();
    let free_one = pool
        .acquire(&mut source, (128, 128), TextureFormat::Rgba8)
        .unwrap();
    let free_two = pool
        .acquire(&mut source, (256, 256), TextureFormat::Rgba8)
        .unwrap();
    pool.release(free_one, 1);
    pool.release(free_two, 2);
    pool.trim(&mut source);
    source.destroyed.sort_unstable();
    assert_eq!(source.destroyed, [2, 3]);
    assert_eq!(pool.bytes(), (2 * MIB, 0));
    pool.release(kept, 3);
    assert_eq!(pool.bytes(), (0, 2 * MIB));
}
