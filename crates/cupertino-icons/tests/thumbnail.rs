//! `thumbnail`: the freedesktop.org thumbnail cache of pictures.

#[allow(dead_code)]
mod support;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cupertino_icons::thumbnail::{file_uri, md5_hex, ThumbError, ThumbSize, Thumbnailer};

struct Setup {
    _dir: tempfile::TempDir,
    cache: PathBuf,
    pictures: PathBuf,
    thumbs: Thumbnailer,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let pictures = dir.path().join("pictures");
    std::fs::create_dir_all(&pictures).unwrap();
    let thumbs = Thumbnailer::new(&cache, "test-app");
    Setup {
        _dir: dir,
        cache,
        pictures,
        thumbs,
    }
}

impl Setup {
    /// A PNG picture of `w` x `h` in the pictures directory.
    fn picture(&self, name: &str, w: u32, h: u32) -> PathBuf {
        let path = self.pictures.join(name);
        support::write_png(&path, w, h, [1, 2, 3, 255]);
        path
    }

    /// A file in the pictures directory that is not a picture.
    fn junk(&self, name: &str) -> PathBuf {
        let path = self.pictures.join(name);
        support::write(&path, b"this is not a picture");
        path
    }
}

/// Move the modification time of `path` `secs` seconds into the future.
fn touch_ahead(path: &Path, secs: u64) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(secs))
        .unwrap();
}

fn chunk(path: &Path, key: &str) -> Option<String> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let reader = decoder.read_info().unwrap();
    reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .find(|c| c.keyword == key)
        .map(|c| c.text.clone())
}

fn dimensions(path: &Path) -> (u32, u32) {
    image::ImageReader::open(path)
        .unwrap()
        .with_guessed_format()
        .unwrap()
        .into_dimensions()
        .unwrap()
}

#[test]
fn the_cache_size_is_the_smallest_that_covers_the_device_pixels() {
    for (points, scale, size) in [
        (64.0, 1.0, ThumbSize::Normal),
        (64.0, 2.0, ThumbSize::Normal),
        (65.0, 2.0, ThumbSize::Large),
        (128.0, 2.0, ThumbSize::Large),
        (256.0, 1.5, ThumbSize::XLarge),
        (2000.0, 2.0, ThumbSize::XXLarge),
        (0.0, 1.0, ThumbSize::Normal),
    ] {
        assert_eq!(
            ThumbSize::for_icon(points, scale),
            size,
            "{points} at {scale}"
        );
    }
}

#[test]
fn a_file_uri_is_percent_encoded_by_segment() {
    for (path, uri) in [
        ("/home/u/a.png", "file:///home/u/a.png"),
        ("/home/u/a b#c%d.png", "file:///home/u/a%20b%23c%25d.png"),
        ("/h/é", "file:///h/%C3%A9"),
        ("/a-b_c.d~e/f", "file:///a-b_c.d~e/f"),
    ] {
        assert_eq!(file_uri(Path::new(path)), uri, "{path}");
    }
}

#[test]
fn a_thumbnail_is_stored_by_size_directory_and_the_md5_of_its_uri() {
    for (text, md5) in [
        ("", "d41d8cd98f00b204e9800998ecf8427e"),
        ("abc", "900150983cd24fb0d6963f7d28e17f72"),
    ] {
        assert_eq!(md5_hex(text), md5, "{text:?}");
    }
    let s = setup();
    let uri = "file:///home/u/a.png";
    assert_eq!(s.thumbs.root(), s.cache.join("thumbnails"));
    for (size, dir) in [
        (ThumbSize::Normal, "normal"),
        (ThumbSize::Large, "large"),
        (ThumbSize::XLarge, "x-large"),
        (ThumbSize::XXLarge, "xx-large"),
    ] {
        assert_eq!(
            s.thumbs.thumb_path(uri, size),
            s.cache
                .join("thumbnails")
                .join(dir)
                .join(format!("{}.png", md5_hex(uri))),
            "{size:?}"
        );
    }
}

#[test]
fn from_env_uses_the_cache_home_of_the_environment() {
    let saved = ["XDG_CACHE_HOME", "HOME"].map(|n| (n, std::env::var_os(n)));
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CACHE_HOME", dir.path());
    assert_eq!(
        Thumbnailer::from_env("app").root(),
        dir.path().join("thumbnails")
    );
    std::env::remove_var("XDG_CACHE_HOME");
    std::env::set_var("HOME", dir.path());
    assert_eq!(
        Thumbnailer::from_env("app").root(),
        dir.path().join(".cache/thumbnails")
    );
    for (name, value) in saved {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

#[test]
fn a_thumbnail_is_made_from_the_content_at_the_size_with_the_standard_text_chunks() {
    let s = setup();
    let jpeg = s.pictures.join("a.jpg");
    image::RgbImage::from_pixel(20, 20, image::Rgb([200, 10, 10]))
        .save_with_format(&jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let webp = s.pictures.join("b.webp");
    image::RgbaImage::from_pixel(20, 20, image::Rgba([10, 10, 200, 255]))
        .save_with_format(&webp, image::ImageFormat::WebP)
        .unwrap();
    for (row, picture, size, dims, mime) in [
        (
            "the aspect is kept",
            s.picture("wide.png", 400, 200),
            ThumbSize::Normal,
            (128, 64),
            "image/png",
        ),
        (
            "a picture that fits is not enlarged",
            s.picture("small.png", 40, 30),
            ThumbSize::Large,
            (40, 30),
            "image/png",
        ),
        ("jpeg", jpeg, ThumbSize::Normal, (20, 20), "image/jpeg"),
        ("webp", webp, ThumbSize::Normal, (20, 20), "image/webp"),
        (
            "the format comes from the content",
            s.picture("actually-a-png.txt", 8, 8),
            ThumbSize::Normal,
            (8, 8),
            "image/png",
        ),
    ] {
        let made = s.thumbs.create(&picture, size).expect(row);
        assert_eq!(
            made,
            s.thumbs.thumb_path(&file_uri(&picture), size),
            "{row}"
        );
        assert_eq!(dimensions(&made), dims, "{row}");
        let meta = std::fs::metadata(&picture).unwrap();
        let (w, h) = dimensions(&picture);
        for (key, value) in [
            ("Thumb::URI", file_uri(&picture)),
            ("Thumb::MTime", meta.mtime().to_string()),
            ("Thumb::Size", meta.len().to_string()),
            ("Thumb::Mimetype", mime.to_string()),
            ("Thumb::Image::Width", w.to_string()),
            ("Thumb::Image::Height", h.to_string()),
        ] {
            assert_eq!(chunk(&made, key), Some(value), "{row}: {key}");
        }
    }
}

#[test]
fn the_cache_is_private_and_holds_no_temporary_files() {
    let s = setup();
    let made = s
        .thumbs
        .create(&s.picture("p.png", 10, 10), ThumbSize::Normal)
        .unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(s.thumbs.root()), 0o700);
    assert_eq!(mode(made.parent().unwrap()), 0o700);
    assert_eq!(mode(&made), 0o600);
    let names: Vec<_> = std::fs::read_dir(made.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [made.file_name().unwrap()]);
}

#[test]
fn a_cached_thumbnail_is_found_only_at_its_size_and_while_the_file_is_unchanged() {
    let s = setup();
    let picture = s.picture("p.png", 300, 300);
    assert_eq!(s.thumbs.lookup(&picture, ThumbSize::Normal), None, "before");
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(s.thumbs.lookup(&picture, ThumbSize::Normal), Some(made));
    assert_eq!(
        s.thumbs.lookup(&picture, ThumbSize::Large),
        None,
        "another size"
    );
    support::write_png(&picture, 310, 310, [9, 9, 9, 255]);
    touch_ahead(&picture, 100);
    assert_eq!(
        s.thumbs.lookup(&picture, ThumbSize::Normal),
        None,
        "changed"
    );
    let gone = s.pictures.join("gone.png");
    assert_eq!(s.thumbs.lookup(&gone, ThumbSize::Normal), None, "missing");
    assert!(!s.thumbs.is_known_failure(&gone));
}

#[test]
fn the_best_cached_thumbnail_is_the_smallest_not_smaller_than_wanted() {
    let s = setup();
    let picture = s.picture("p.png", 2000, 2000);
    assert_eq!(s.thumbs.lookup_at_least(&picture, ThumbSize::Normal), None);
    s.thumbs.create(&picture, ThumbSize::XLarge).unwrap();
    s.thumbs.create(&picture, ThumbSize::XXLarge).unwrap();
    let best = |want| s.thumbs.lookup_at_least(&picture, want).unwrap().0;
    assert_eq!(best(ThumbSize::Normal), ThumbSize::XLarge);
    assert_eq!(best(ThumbSize::XXLarge), ThumbSize::XXLarge);
    s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(best(ThumbSize::Normal), ThumbSize::Normal);
}

#[test]
fn get_or_create_makes_the_thumbnail_once_and_then_returns_the_cached_one() {
    let s = setup();
    let picture = s.picture("p.png", 300, 300);
    let first = s.thumbs.get_or_create(&picture, ThumbSize::Normal).unwrap();
    let stamp = std::fs::metadata(&first).unwrap().modified().unwrap();
    let second = s.thumbs.get_or_create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(first, second);
    assert_eq!(
        std::fs::metadata(&second).unwrap().modified().unwrap(),
        stamp,
        "not made again"
    );
}

#[test]
fn a_failure_is_remembered_for_the_application_until_the_file_changes() {
    let s = setup();
    let file = s.junk("notes.png");
    let other = Thumbnailer::new(&s.cache, "other-app");
    assert!(!s.thumbs.is_known_failure(&file));
    assert!(matches!(
        s.thumbs.create(&file, ThumbSize::Normal),
        Err(ThumbError::Image(_))
    ));
    assert!(s.thumbs.is_known_failure(&file));
    assert!(
        !other.is_known_failure(&file),
        "another application's marker is its own"
    );
    let marker = s
        .thumbs
        .root()
        .join("fail/test-app")
        .join(format!("{}.png", md5_hex(&file_uri(&file))));
    assert_eq!(chunk(&marker, "Thumb::URI"), Some(file_uri(&file)));
    assert!(chunk(&marker, "Thumb::MTime").is_some());
    assert_eq!(s.thumbs.lookup(&file, ThumbSize::Normal), None);
    for result in [
        s.thumbs.create(&file, ThumbSize::Large),
        s.thumbs.get_or_create(&file, ThumbSize::Large),
    ] {
        assert!(
            matches!(result, Err(ThumbError::KnownFailure)),
            "{result:?}"
        );
    }

    support::write_png(&file, 8, 8, [1, 2, 3, 255]);
    touch_ahead(&file, 50);
    assert!(!s.thumbs.is_known_failure(&file), "the file changed");
    assert!(s.thumbs.create(&file, ThumbSize::Normal).is_ok());
}

#[test]
fn a_path_that_is_not_a_readable_picture_outside_the_cache_is_refused() {
    let s = setup();
    let made = s
        .thumbs
        .create(&s.picture("p.png", 300, 300), ThumbSize::Normal)
        .unwrap();
    let link = s.pictures.join("link.png");
    std::os::unix::fs::symlink(&made, &link).unwrap();
    let fifo = s.pictures.join("pipe.png");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let wide = s.picture("wide.png", 8193, 1);
    for (row, path, image_error) in [
        ("a directory", s.pictures.clone(), false),
        ("a missing path", s.pictures.join("none.png"), false),
        ("a file in the cache", made.clone(), false),
        ("a link into the cache", link.clone(), false),
        ("a pipe is not read", fifo, false),
        ("wider than 8192 pixels", wide, true),
    ] {
        match s.thumbs.create(&path, ThumbSize::Large) {
            Err(ThumbError::Io(_)) if !image_error => {}
            Err(ThumbError::Image(_)) if image_error => {}
            other => panic!("{row}: {other:?}"),
        }
    }
    assert!(
        s.thumbs
            .create(&s.picture("ok.png", 8192, 1), ThumbSize::Normal)
            .is_ok(),
        "8192 wide"
    );
    assert!(s.thumbs.is_in_cache(&made) && s.thumbs.is_in_cache(&link));
    assert!(!s.thumbs.is_in_cache(&s.pictures.join("p.png")));
    assert!(!s.thumbs.is_in_cache(&s.pictures.join("none")));
    // refusing a cache file leaves no failure marker
    for path in [&made, &link] {
        let marker = s
            .thumbs
            .root()
            .join("fail/test-app")
            .join(format!("{}.png", md5_hex(&file_uri(path))));
        assert!(!marker.exists(), "{}", path.display());
    }
}

#[test]
fn pixels_are_the_best_cached_or_a_new_thumbnail_shrunk_to_the_edge_and_never_enlarged() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 600, 300, [200, 100, 50, 255]);
    let px = s.thumbs.pixels(&picture, ThumbSize::Normal, 64).unwrap();
    assert_eq!((px.width, px.height), (64, 32));
    assert_eq!(px.rgba.len(), 64 * 32 * 4);
    assert_eq!(
        &px.rgba[..4],
        &[200, 100, 50, 255],
        "straight, not premultiplied"
    );
    assert!(
        s.thumbs.lookup(&picture, ThumbSize::Normal).is_some(),
        "created on the way"
    );
    let px = s.thumbs.pixels(&picture, ThumbSize::Normal, 1000).unwrap();
    assert_eq!(
        (px.width, px.height),
        (128, 64),
        "not enlarged past the thumbnail"
    );

    let large = s.picture("large.png", 2000, 1000);
    s.thumbs.create(&large, ThumbSize::Large).unwrap();
    let px = s.thumbs.pixels(&large, ThumbSize::Normal, 1000).unwrap();
    assert_eq!((px.width, px.height), (256, 128), "the larger cached one");
    assert_eq!(
        s.thumbs.lookup(&large, ThumbSize::Normal),
        None,
        "none made at the smaller size"
    );

    assert!(s
        .thumbs
        .pixels(&s.junk("bad.png"), ThumbSize::Normal, 64)
        .is_err());
}
