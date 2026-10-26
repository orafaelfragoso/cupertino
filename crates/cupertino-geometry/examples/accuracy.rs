//! Prints the deviation of the cubic path approximation from the design signed distance.
use cupertino_geometry::rounded::PathOptions;
use cupertino_geometry::{Rect, RoundedRect};

fn main() {
    let r = RoundedRect::continuous(Rect::new(0.0, 0.0, 400.0, 300.0), 100.0);
    for segs in [1usize, 2, 3, 4, 6] {
        let path = r.path_with(&PathOptions {
            segments_per_corner: segs,
        });
        let worst = path
            .flatten(0.001)
            .iter()
            .map(|&p| r.sdf(p).abs())
            .fold(0.0, f64::max);
        println!(
            "segments/corner {segs}: max path deviation {:.5} px at radius 100 ({:.2e} of radius)",
            worst,
            worst / 100.0
        );
    }
}
