//! Pictures read from files: a wallpaper (`wallpaper::decode`) and a user's picture (`user::load_picture`), PNG
//! or JPEG, and what is refused: a FIFO, a file past the size given, another format.

// a test writes its files on its own thread, as the I/O pool would read them
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;
use std::sync::Arc;

use cupertino_kit::image::{encode_jpeg, encode_png};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_login::user::load_picture;
use cupertino_login::wallpaper::{self, Source, MAX_FILE_BYTES};

/// The test pictures' pixels.
const WIDTH: u32 = 4;
const HEIGHT: u32 = 3;

/// A directory of the test's own under the build's directory for test files.
fn directory(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("login-decode")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The opaque colour of pixel \`(x, y)\` of the test picture.
fn colour(x: u32, y: u32) -> [u8; 3] {
    [(x * 60) as u8, (y * 80) as u8, 200]
}

fn picture() -> ImageResource {
    let pixels: Vec<u8> = (0..HEIGHT)
        .flat_map(|y| {
            (0..WIDTH).flat_map(move |x| {
                let [r, g, b] = colour(x, y);
                [r, g, b, 255]
            })
        })
        .collect();
    ImageResource {
        revision: 1,
        width: WIDTH,
        height: HEIGHT,
        stride: WIDTH * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: Arc::from(pixels),
    }
}

#[test]
fn a_png_wallpaper_decodes_to_its_own_colours() {
    let dir = directory("png");
    let path = dir.join("wallpaper.png");
    std::fs::write(&path, encode_png(&picture()).unwrap()).unwrap();
    let decoded = wallpaper::decode(&path, MAX_FILE_BYTES).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
    // drawn at its own size with no dimming, each pixel is the picture's
    let source = Source::Picture(Arc::new(decoded));
    let drawn = wallpaper::render(&source, WIDTH, HEIGHT, 1.0, 0.0);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let at = ((y * WIDTH + x) * 4) as usize;
            let [r, g, b] = colour(x, y);
            assert_eq!(drawn[at..at + 4], [r, g, b, 255], "pixel {x}, {y}");
        }
    }
}

#[test]
fn a_jpeg_wallpaper_decodes() {
    let dir = directory("jpeg");
    let path = dir.join("wallpaper.jpg");
    std::fs::write(&path, encode_jpeg(&picture(), 90).unwrap()).unwrap();
    let decoded = wallpaper::decode(&path, MAX_FILE_BYTES).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
}

#[test]
fn a_file_past_the_size_given_is_refused() {
    let dir = directory("large");
    let path = dir.join("wallpaper.png");
    let bytes = encode_png(&picture()).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let refused = wallpaper::decode(&path, bytes.len() as u64 - 1).unwrap_err();
    assert!(refused.contains("not a picture file"), "{refused}");
}

#[test]
fn another_format_is_refused() {
    let dir = directory("gif");
    let path = dir.join("wallpaper.gif");
    std::fs::write(&path, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
    let refused = wallpaper::decode(&path, MAX_FILE_BYTES).unwrap_err();
    assert!(refused.contains("only PNG and JPEG"), "{refused}");
}

#[test]
fn a_fifo_is_refused_without_blocking() {
    let dir = directory("fifo");
    let path = dir.join("wallpaper.png");
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .unwrap();
    assert!(made.success());
    // no writer: an open that blocked would never return
    let refused = wallpaper::decode(&path, MAX_FILE_BYTES).unwrap_err();
    assert!(refused.contains("not a picture file"), "{refused}");
    assert!(load_picture(&path).is_err());
}

#[test]
fn a_users_face_file_is_their_picture() {
    let dir = directory("face");
    let path = dir.join(".face");
    assert!(
        load_picture(&path).unwrap().is_none(),
        "no file, no picture"
    );
    std::fs::write(&path, encode_png(&picture()).unwrap()).unwrap();
    let face = load_picture(&path).unwrap().expect("the picture");
    assert_eq!((face.image.width, face.image.height), (WIDTH, HEIGHT));
    assert_eq!(face.image.format, PixelFormat::Rgba8Premultiplied);
    let [r, g, b] = colour(1, 2);
    let at = (2 * face.image.stride + 4) as usize;
    assert_eq!(face.image.pixels[at..at + 4], [r, g, b, 255]);
}
