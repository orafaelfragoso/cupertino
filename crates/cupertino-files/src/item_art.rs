//! The images Files draws itself for its most common items: a folder, a text document (a page of greeked lines, as
//! a thumbnail of its text would look), another document (a page with a folded corner) and an archive (that page with
//! a zipper). They are drawn from shapes with antialiased edges at any size, so every scale gets sharp pixels and no
//! image file is needed; an asset pack that ships its own icon for the kind is drawn instead (`kit::item_icons`).
//!
//! The shapes are laid out in fractions of the icon's square, so the art fills the same share of its box at every size.

/// What [`draw`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Art {
    Folder,
    Text,
    Document,
    Archive,
}

impl Art {
    /// The art that stands for the semantic icon name `name` (a kind of `cupertino_kit::file_icons`, or a special
    /// folder's own name), `None` for a kind drawn from the icon theme (pictures, sound, video, PDF, programs).
    pub fn for_icon(name: &str) -> Option<Art> {
        match name {
            "document.text" => Some(Art::Text),
            "document.generic" => Some(Art::Document),
            "document.archive" => Some(Art::Archive),
            "folder" => Some(Art::Folder),
            n if n.starts_with("folder.") => Some(Art::Folder),
            _ => None,
        }
    }
}

/// An sRGB colour with straight alpha, 0–255.
type Rgba = [u8; 4];

/// The folder: its back panel and tab, the sheet that shows above its front, and the front's top and bottom colours.
const FOLDER_BACK: Rgba = [78, 178, 226, 255];
const FOLDER_SHEET: Rgba = [255, 255, 255, 255];
const FOLDER_FRONT_TOP: Rgba = [112, 205, 246, 255];
const FOLDER_FRONT_BOTTOM: Rgba = [69, 172, 220, 255];
/// The thin light line along the top of the folder's front.
const FOLDER_RIM: Rgba = [150, 222, 252, 255];
/// The folder's extent in the square: left, top, right, bottom, then the top of its front and the right edge and
/// bottom of its tab, and its corner radius.
const FOLDER: [f64; 4] = [0.043, 0.134, 0.957, 0.866];
const FOLDER_FRONT: f64 = 0.275;
const FOLDER_TAB: (f64, f64) = (0.36, 0.25);
const FOLDER_BACK_TOP: f64 = 0.19;
const FOLDER_RADIUS: f64 = 0.045;

/// A page: the text page's extent and the other pages' (left, top, right, bottom), the folded corner's side, the
/// corner radius, and the colours of a page, its edge, its fold and its shadow.
const TEXT_PAGE: [f64; 4] = [0.175, 0.07, 0.825, 0.93];
const PAGE: [f64; 4] = [0.185, 0.075, 0.815, 0.925];
const FOLD: f64 = 0.19;
const PAGE_RADIUS: f64 = 0.025;
const PAGE_FILL: Rgba = [255, 255, 255, 255];
const DOCUMENT_FILL: Rgba = [245, 245, 245, 255];
const PAGE_EDGE: Rgba = [0, 0, 0, 46];
const FOLD_FILL: Rgba = [222, 222, 222, 255];
const SHADOW: Rgba = [0, 0, 0, 56];
/// The greeked lines of a text page: their margin, pitch and thickness as shares of the square, and their colour.
const LINE_MARGIN: f64 = 0.05;
const LINE_PITCH: f64 = 0.03;
const LINE_THICKNESS: f64 = 0.012;
const LINE: Rgba = [0, 0, 0, 120];
/// The zipper of an archive: its teeth's width and pitch, how far down the page it runs, and its colour.
const ZIPPER_TOOTH: (f64, f64) = (0.045, 0.028);
const ZIPPER_LENGTH: f64 = 0.42;
const ZIPPER: Rgba = [168, 168, 168, 255];

/// `art` drawn `px` device pixels square, as premultiplied RGBA rows; `seed` varies a text page's lines, so pages
/// of different files do not all look the same.
pub fn draw(art: Art, px: u32, seed: u64) -> Vec<u8> {
    let mut canvas = Canvas::new(px);
    match art {
        Art::Folder => folder(&mut canvas),
        Art::Text => text_page(&mut canvas, seed),
        Art::Document => page(&mut canvas, DOCUMENT_FILL),
        Art::Archive => {
            page(&mut canvas, DOCUMENT_FILL);
            zipper(&mut canvas);
        }
    }
    canvas.pixels
}

fn folder(c: &mut Canvas) {
    let [l, t, r, b] = FOLDER;
    let radius = FOLDER_RADIUS;
    c.fill(
        &RoundRect::new(l, t, FOLDER_TAB.0, FOLDER_TAB.1, radius),
        FOLDER_BACK,
    );
    c.fill(
        &RoundRect::new(l, FOLDER_BACK_TOP, r, b, radius),
        FOLDER_BACK,
    );
    c.fill(
        &RoundRect::new(
            l + 0.035,
            FOLDER_BACK_TOP + 0.045,
            r - 0.035,
            b - 0.1,
            radius,
        ),
        FOLDER_SHEET,
    );
    let front = RoundRect::new(l, FOLDER_FRONT, r, b, radius);
    let rim = RoundRect::new(l, FOLDER_FRONT - 0.012, r, b, radius);
    c.fill(&rim, FOLDER_RIM);
    c.fill_gradient(
        &front,
        FOLDER_FRONT_TOP,
        FOLDER_FRONT_BOTTOM,
        (FOLDER_FRONT, b),
    );
}

/// A page with a folded top-right corner in `fill`, over its shadow.
fn page(c: &mut Canvas, fill: Rgba) {
    let [l, t, r, b] = PAGE;
    let body = RoundRect::new(l, t, r, b, PAGE_RADIUS);
    let cut = Corner {
        right: r,
        top: t,
        side: FOLD,
    };
    let shadow = RoundRect::new(l, t + 0.01, r, b + 0.012, PAGE_RADIUS);
    c.fill(&Shadowed(&Cut(&shadow, cut, 0.0)), SHADOW);
    c.fill(&Cut(&body, cut, 0.0), PAGE_EDGE);
    let inset = 1.0 / c.side;
    c.fill(
        &Cut(
            &RoundRect::new(l + inset, t + inset, r - inset, b - inset, PAGE_RADIUS),
            cut,
            inset,
        ),
        fill,
    );
    c.fill(&Flap(cut), FOLD_FILL);
}

/// A page of greeked lines, without a fold, as a text file's thumbnail.
fn text_page(c: &mut Canvas, seed: u64) {
    let [l, t, r, b] = TEXT_PAGE;
    c.fill(
        &Shadowed(&RoundRect::new(l, t + 0.01, r, b + 0.012, PAGE_RADIUS)),
        SHADOW,
    );
    c.fill(&RoundRect::new(l, t, r, b, PAGE_RADIUS), PAGE_EDGE);
    let inset = 1.0 / c.side;
    c.fill(
        &RoundRect::new(l + inset, t + inset, r - inset, b - inset, PAGE_RADIUS),
        PAGE_FILL,
    );
    // lines thinner than half a pixel would vanish: a small icon shows fewer, thicker ones
    let thickness = LINE_THICKNESS.max(1.0 / c.side);
    let pitch = LINE_PITCH.max(2.5 / c.side);
    let (left, right) = (l + LINE_MARGIN, r - LINE_MARGIN);
    let mut state = seed | 1;
    let mut y = t + LINE_MARGIN;
    while y + thickness < b - LINE_MARGIN {
        // a short line now and then ends a paragraph, and the next leaves a blank line
        let roll = next(&mut state);
        let share = if roll % 7 == 0 {
            0.25 + f64::from((roll >> 8) as u8) / 255.0 * 0.4
        } else {
            0.82 + f64::from((roll >> 16) as u8) / 255.0 * 0.18
        };
        c.fill(
            &RoundRect::new(left, y, left + (right - left) * share, y + thickness, 0.0),
            LINE,
        );
        y += pitch;
        if roll % 7 == 0 {
            y += pitch;
        }
    }
}

/// The zipper down the middle of a page from its top.
fn zipper(c: &mut Canvas) {
    let [l, t, r, _] = PAGE;
    let centre = (l + r) / 2.0;
    let (width, pitch) = ZIPPER_TOOTH;
    let mut y = t + 0.01;
    let mut left = true;
    while y < t + ZIPPER_LENGTH {
        let x = if left { centre - width } else { centre };
        c.fill(
            &RoundRect::new(x, y, x + width, y + pitch * 0.6, 0.0),
            ZIPPER,
        );
        y += pitch;
        left = !left;
    }
    c.fill(
        &RoundRect::new(
            centre - width,
            y,
            centre + width,
            y + 2.5 * pitch,
            width * 0.5,
        ),
        ZIPPER,
    );
}

/// A step of a small deterministic generator (xorshift).
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// A shape, as the signed distance (in units of the square) from a point to its edge, negative inside.
trait Shape {
    fn distance(&self, x: f64, y: f64) -> f64;
    /// How far outside the edge coverage fades, in device pixels: 1 for a crisp edge.
    fn softness(&self) -> f64 {
        1.0
    }
}

struct RoundRect {
    centre: (f64, f64),
    half: (f64, f64),
    radius: f64,
}

impl RoundRect {
    fn new(left: f64, top: f64, right: f64, bottom: f64, radius: f64) -> Self {
        let half = ((right - left) / 2.0, (bottom - top) / 2.0);
        RoundRect {
            centre: ((left + right) / 2.0, (top + bottom) / 2.0),
            half,
            radius: radius.min(half.0).min(half.1),
        }
    }
}

impl Shape for RoundRect {
    fn distance(&self, x: f64, y: f64) -> f64 {
        let qx = (x - self.centre.0).abs() - self.half.0 + self.radius;
        let qy = (y - self.centre.1).abs() - self.half.1 + self.radius;
        let outside = qx.max(0.0).hypot(qy.max(0.0));
        outside + qx.max(qy).min(0.0) - self.radius
    }
}

/// The top-right corner a page's fold takes: the page's right edge and top, and the fold's side.
#[derive(Clone, Copy)]
struct Corner {
    right: f64,
    top: f64,
    side: f64,
}

impl Corner {
    /// Distance to the fold's diagonal, positive beyond it (towards the corner).
    fn beyond(&self, x: f64, y: f64) -> f64 {
        ((x - (self.right - self.side)) - (y - self.top)) / std::f64::consts::SQRT_2
    }
}

/// A shape with its corner cut off along the fold's diagonal, moved in by `inset`.
struct Cut<'a>(&'a RoundRect, Corner, f64);

impl Shape for Cut<'_> {
    fn distance(&self, x: f64, y: f64) -> f64 {
        self.0.distance(x, y).max(self.1.beyond(x, y) + self.2)
    }
}

/// The folded flap: the triangle under the fold's diagonal in the corner's square.
struct Flap(Corner);

impl Shape for Flap {
    fn distance(&self, x: f64, y: f64) -> f64 {
        let c = self.0;
        let left = (c.right - c.side) - x;
        let bottom = y - (c.top + c.side);
        left.max(bottom).max(c.beyond(x, y))
    }
}

/// A shape's soft shadow: its edge fades over a few pixels.
struct Shadowed<'a>(&'a dyn Shape);

impl Shape for Shadowed<'_> {
    fn distance(&self, x: f64, y: f64) -> f64 {
        self.0.distance(x, y)
    }
    fn softness(&self) -> f64 {
        3.0
    }
}

/// Premultiplied RGBA pixels, `side` device pixels square.
struct Canvas {
    side: f64,
    px: u32,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(px: u32) -> Self {
        let px = px.max(1);
        Canvas {
            side: f64::from(px),
            px,
            pixels: vec![0; (px * px * 4) as usize],
        }
    }

    fn fill(&mut self, shape: &dyn Shape, color: Rgba) {
        self.paint(shape, |_| color);
    }

    /// `shape` in a vertical gradient from `top` to `bottom` colour over `span` (fractions of the square).
    fn fill_gradient(&mut self, shape: &dyn Shape, top: Rgba, bottom: Rgba, span: (f64, f64)) {
        self.paint(shape, |y| {
            let t = ((y - span.0) / (span.1 - span.0)).clamp(0.0, 1.0);
            let mix =
                |a: u8, b: u8| (f64::from(a) + (f64::from(b) - f64::from(a)) * t).round() as u8;
            [
                mix(top[0], bottom[0]),
                mix(top[1], bottom[1]),
                mix(top[2], bottom[2]),
                mix(top[3], bottom[3]),
            ]
        });
    }

    /// Each pixel covered by `shape`, blended over what is there in the colour `color` gives at its height.
    fn paint(&mut self, shape: &dyn Shape, color: impl Fn(f64) -> Rgba) {
        let soft = shape.softness();
        for row in 0..self.px {
            let y = (f64::from(row) + 0.5) / self.side;
            let c = color(y);
            for col in 0..self.px {
                let x = (f64::from(col) + 0.5) / self.side;
                let d = shape.distance(x, y) * self.side;
                let coverage = ((soft * 0.5 - d) / soft).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let alpha = f64::from(c[3]) / 255.0 * coverage;
                let i = ((row * self.px + col) * 4) as usize;
                for k in 0..3 {
                    let src = f64::from(c[k]) * alpha;
                    let dst = f64::from(self.pixels[i + k]);
                    self.pixels[i + k] = (src + dst * (1.0 - alpha)).round() as u8;
                }
                let dst = f64::from(self.pixels[i + 3]);
                self.pixels[i + 3] = (alpha * 255.0 + dst * (1.0 - alpha)).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pixels: &[u8], px: u32, x: f64, y: f64) -> [u8; 4] {
        let (col, row) = ((x * f64::from(px)) as u32, (y * f64::from(px)) as u32);
        let i = ((row * px + col) * 4) as usize;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    }

    #[test]
    fn the_kinds_drawn_here_are_folders_text_other_documents_and_archives() {
        assert_eq!(Art::for_icon("folder"), Some(Art::Folder));
        assert_eq!(Art::for_icon("folder.documents"), Some(Art::Folder));
        assert_eq!(Art::for_icon("document.text"), Some(Art::Text));
        assert_eq!(Art::for_icon("document.generic"), Some(Art::Document));
        assert_eq!(Art::for_icon("document.archive"), Some(Art::Archive));
        for theme in [
            "document.image",
            "document.pdf",
            "document.video",
            "computer",
        ] {
            assert_eq!(Art::for_icon(theme), None, "{theme}");
        }
    }

    #[test]
    fn a_folder_is_a_blue_front_over_its_back_and_leaves_its_corners_clear() {
        let px = 128;
        let art = draw(Art::Folder, px, 0);
        assert_eq!(art.len(), (px * px * 4) as usize);
        assert_eq!(pixel(&art, px, 0.01, 0.01), [0; 4], "outside the folder");
        assert_eq!(pixel(&art, px, 0.9, 0.15), [0; 4], "beside the tab");
        let front = pixel(&art, px, 0.5, 0.6);
        assert_eq!(front[3], 255);
        assert!(front[2] > front[0] && front[2] > 200, "blue: {front:?}");
        let tab = pixel(&art, px, 0.2, 0.16);
        assert_eq!(tab, FOLDER_BACK, "the tab");
        let sheet = pixel(&art, px, 0.5, 0.25);
        assert_eq!(sheet, FOLDER_SHEET, "the sheet shows above the front");
        // the front darkens towards the bottom
        assert!(pixel(&art, px, 0.5, 0.84)[1] < pixel(&art, px, 0.5, 0.32)[1]);
    }

    #[test]
    fn a_document_is_a_page_with_its_corner_folded_and_an_archive_adds_a_zipper() {
        let px = 128;
        let document = draw(Art::Document, px, 0);
        assert_eq!(pixel(&document, px, 0.5, 0.6), DOCUMENT_FILL);
        // the corner beyond the fold is clear of the page (a little of the shadow may show)
        assert!(pixel(&document, px, 0.81, 0.08)[3] < 80);
        assert_eq!(pixel(&document, px, 0.7, 0.2), FOLD_FILL, "the flap");
        let archive = draw(Art::Archive, px, 0);
        let tooth = (0..40)
            .map(|k| pixel(&archive, px, 0.48, 0.09 + f64::from(k) * 0.01))
            .any(|p| p == ZIPPER);
        assert!(tooth, "the zipper runs down the middle");
        assert_eq!(pixel(&archive, px, 0.3, 0.6), DOCUMENT_FILL);
    }

    #[test]
    fn a_text_page_holds_lines_that_vary_with_the_seed_and_stays_inside_its_square() {
        let px = 128;
        let a = draw(Art::Text, px, 1);
        let b = draw(Art::Text, px, 2);
        assert_ne!(a, b);
        assert_eq!(
            draw(Art::Text, px, 1),
            a,
            "the same seed draws the same page"
        );
        let inked = (0..px)
            .filter(|&row| {
                let p = pixel(&a, px, 0.3, (f64::from(row) + 0.5) / f64::from(px));
                p[3] == 255 && p[0] < 200
            })
            .count();
        assert!(inked > 10, "lines of text: {inked}");
        assert_eq!(pixel(&a, px, 0.05, 0.5), [0; 4], "beside the page");
        // a tiny icon still draws
        assert_eq!(draw(Art::Text, 16, 1).len(), 16 * 16 * 4);
    }
}
