//! `kit::render::zeroed_pixels`: pixel memory a program fills in place.

use cupertino_kit::render::zeroed_pixels;
use std::sync::Arc;

#[test]
fn zeroed_pixels_are_zero_and_unshared() {
    let mut pixels = zeroed_pixels(4 * 64 * 64);
    assert_eq!(pixels.len(), 4 * 64 * 64);
    assert!(pixels.iter().all(|&b| b == 0));
    let out = Arc::get_mut(&mut pixels).expect("no other handle");
    out[0] = 255;
    assert_eq!(pixels[0], 255);
    assert!(zeroed_pixels(0).is_empty());
}
