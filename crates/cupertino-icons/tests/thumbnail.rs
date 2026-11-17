//! `thumbnail`: the freedesktop.org thumbnail cache of pictures.

#[allow(dead_code)]
mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

fn chunks(path: &Path) -> Vec<(String, String)> {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let reader = decoder.read_info().unwrap();
    reader
        .info()
        .uncompressed_latin1_text
        .iter()
        .map(|c| (c.keyword.clone(), c.text.clone()))
        .collect()
}

fn chunk(path: &Path, key: &str) -> Option<String> {
    chunks(path)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn dimensions(path: &Path) -> (u32, u32) {
    let img = image::open(path).unwrap();
    (img.width(), img.height())
}

#[test]
fn the_sizes_are_128_256_512_and_1024_pixels_in_their_directories() {
    assert_eq!(ThumbSize::ALL.map(|s| s.edge()), [128, 256, 512, 1024]);
    assert_eq!(
        ThumbSize::ALL.map(|s| s.dir()),
        ["normal", "large", "x-large", "xx-large"]
    );
    assert!(ThumbSize::Normal < ThumbSize::Large && ThumbSize::XLarge < ThumbSize::XXLarge);
}

#[test]
fn the_cache_size_is_the_smallest_that_covers_the_device_pixels() {
    assert_eq!(ThumbSize::for_icon(64.0, 1.0), ThumbSize::Normal);
    assert_eq!(
        ThumbSize::for_icon(64.0, 2.0),
        ThumbSize::Normal,
        "128 pixels fit exactly"
    );
    assert_eq!(ThumbSize::for_icon(65.0, 2.0), ThumbSize::Large);
    assert_eq!(ThumbSize::for_icon(128.0, 2.0), ThumbSize::Large);
    assert_eq!(ThumbSize::for_icon(256.0, 1.5), ThumbSize::XLarge);
    assert_eq!(
        ThumbSize::for_icon(2000.0, 2.0),
        ThumbSize::XXLarge,
        "never larger than the largest"
    );
    assert_eq!(ThumbSize::for_icon(0.0, 1.0), ThumbSize::Normal);
}

#[test]
fn a_file_uri_is_percent_encoded_by_segment() {
    assert_eq!(file_uri(Path::new("/home/u/a.png")), "file:///home/u/a.png");
    assert_eq!(
        file_uri(Path::new("/home/u/a b#c%d.png")),
        "file:///home/u/a%20b%23c%25d.png"
    );
    assert_eq!(file_uri(Path::new("/h/é")), "file:///h/%C3%A9");
    assert_eq!(
        file_uri(Path::new("/a-b_c.d~e/f")),
        "file:///a-b_c.d~e/f",
        "unreserved characters stay"
    );
}

#[test]
fn the_name_of_a_thumbnail_is_the_md5_of_the_uri() {
    assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
    assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
}

#[test]
fn a_thumbnail_is_stored_under_thumbnails_by_size_and_hash() {
    let s = setup();
    let uri = "file:///home/u/a.png";
    assert_eq!(s.thumbs.root(), s.cache.join("thumbnails"));
    assert_eq!(
        s.thumbs.thumb_path(uri, ThumbSize::XLarge),
        s.cache
            .join("thumbnails/x-large")
            .join(format!("{}.png", md5_hex(uri)))
    );
}

#[test]
fn from_env_uses_the_cache_home_of_the_environment() {
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
}

#[test]
fn a_thumbnail_is_made_at_the_size_with_the_aspect_kept_and_the_standard_text_chunks() {
    let s = setup();
    let picture = s.pictures.join("wide.png");
    support::write_png(&picture, 400, 200, [10, 20, 30, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(
        made,
        s.thumbs.thumb_path(&file_uri(&picture), ThumbSize::Normal)
    );
    assert_eq!(dimensions(&made), (128, 64));
    assert_eq!(chunk(&made, "Thumb::URI").unwrap(), file_uri(&picture));
    let meta = std::fs::metadata(&picture).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(
        chunk(&made, "Thumb::MTime").unwrap(),
        meta.mtime().to_string()
    );
    assert_eq!(chunk(&made, "Thumb::Size").unwrap(), meta.len().to_string());
    assert_eq!(chunk(&made, "Thumb::Mimetype").unwrap(), "image/png");
    assert_eq!(chunk(&made, "Thumb::Image::Width").unwrap(), "400");
    assert_eq!(chunk(&made, "Thumb::Image::Height").unwrap(), "200");
}

#[test]
fn a_picture_that_fits_is_not_enlarged() {
    let s = setup();
    let picture = s.pictures.join("small.png");
    support::write_png(&picture, 40, 30, [1, 2, 3, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Large).unwrap();
    assert_eq!(dimensions(&made), (40, 30));
}

#[test]
fn the_cache_directories_and_files_are_private() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 10, 10, [1, 2, 3, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(s.thumbs.root()), 0o700);
    assert_eq!(mode(made.parent().unwrap()), 0o700);
    assert_eq!(mode(&made), 0o600);
}

#[test]
fn creating_leaves_no_temporary_files_in_the_cache() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 10, 10, [1, 2, 3, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    let names: Vec<_> = std::fs::read_dir(made.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, [made.file_name().unwrap()]);
}

#[test]
fn jpeg_and_webp_pictures_are_thumbnailed_with_their_mime_type() {
    let s = setup();
    let jpeg = s.pictures.join("a.jpg");
    let webp = s.pictures.join("b.webp");
    let img = image::RgbImage::from_pixel(20, 20, image::Rgb([200, 10, 10]));
    img.save_with_format(&jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    image::RgbaImage::from_pixel(20, 20, image::Rgba([10, 10, 200, 255]))
        .save_with_format(&webp, image::ImageFormat::WebP)
        .unwrap();
    let a = s.thumbs.create(&jpeg, ThumbSize::Normal).unwrap();
    let b = s.thumbs.create(&webp, ThumbSize::Normal).unwrap();
    assert_eq!(chunk(&a, "Thumb::Mimetype").unwrap(), "image/jpeg");
    assert_eq!(chunk(&b, "Thumb::Mimetype").unwrap(), "image/webp");
}

#[test]
fn the_format_comes_from_the_content_not_the_name() {
    let s = setup();
    let picture = s.pictures.join("actually-a-png.txt");
    support::write_png(&picture, 8, 8, [1, 2, 3, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(chunk(&made, "Thumb::Mimetype").unwrap(), "image/png");
}

#[test]
fn a_cached_thumbnail_is_found_while_the_file_is_unchanged() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 300, 300, [1, 2, 3, 255]);
    assert_eq!(s.thumbs.lookup(&picture, ThumbSize::Normal), None);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(s.thumbs.lookup(&picture, ThumbSize::Normal), Some(made));
    assert_eq!(
        s.thumbs.lookup(&picture, ThumbSize::Large),
        None,
        "another size is another file"
    );
}

#[test]
fn a_thumbnail_of_a_changed_file_is_no_longer_valid() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 300, 300, [1, 2, 3, 255]);
    s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    // a different size and modification time
    support::write_png(&picture, 310, 310, [9, 9, 9, 255]);
    let file = std::fs::File::options().write(true).open(&picture).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(100))
        .unwrap();
    assert_eq!(s.thumbs.lookup(&picture, ThumbSize::Normal), None);
}

#[test]
fn a_file_that_does_not_exist_has_no_cached_thumbnail() {
    let s = setup();
    assert_eq!(
        s.thumbs
            .lookup(&s.pictures.join("gone.png"), ThumbSize::Normal),
        None
    );
    assert!(!s.thumbs.is_known_failure(&s.pictures.join("gone.png")));
}

#[test]
fn the_best_cached_thumbnail_is_the_smallest_not_smaller_than_wanted() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 2000, 2000, [1, 2, 3, 255]);
    assert_eq!(s.thumbs.lookup_at_least(&picture, ThumbSize::Normal), None);
    s.thumbs.create(&picture, ThumbSize::XLarge).unwrap();
    s.thumbs.create(&picture, ThumbSize::XXLarge).unwrap();
    let (size, _) = s
        .thumbs
        .lookup_at_least(&picture, ThumbSize::Normal)
        .unwrap();
    assert_eq!(size, ThumbSize::XLarge);
    let (size, _) = s
        .thumbs
        .lookup_at_least(&picture, ThumbSize::XXLarge)
        .unwrap();
    assert_eq!(size, ThumbSize::XXLarge);
    s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert_eq!(
        s.thumbs
            .lookup_at_least(&picture, ThumbSize::Normal)
            .unwrap()
            .0,
        ThumbSize::Normal
    );
}

#[test]
fn get_or_create_makes_the_thumbnail_once_and_then_returns_the_cached_one() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 300, 300, [1, 2, 3, 255]);
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
fn a_file_that_is_not_a_picture_fails_and_the_failure_is_remembered_under_the_application() {
    let s = setup();
    let file = s.pictures.join("notes.png");
    support::write(&file, b"this is not a picture");
    assert!(!s.thumbs.is_known_failure(&file));
    assert!(matches!(
        s.thumbs.create(&file, ThumbSize::Normal),
        Err(ThumbError::Image(_))
    ));
    assert!(s.thumbs.is_known_failure(&file));
    let marker = s
        .thumbs
        .root()
        .join("fail/test-app")
        .join(format!("{}.png", md5_hex(&file_uri(&file))));
    assert_eq!(chunk(&marker, "Thumb::URI").unwrap(), file_uri(&file));
    assert!(chunk(&marker, "Thumb::MTime").is_some());
    assert_eq!(s.thumbs.lookup(&file, ThumbSize::Normal), None);
}

#[test]
fn a_file_that_changed_after_its_failure_is_tried_again() {
    let s = setup();
    let file = s.pictures.join("later.png");
    support::write(&file, b"not yet");
    assert!(s.thumbs.create(&file, ThumbSize::Normal).is_err());
    support::write_png(&file, 8, 8, [1, 2, 3, 255]);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(50))
        .unwrap();
    // the file changed: its failure is no longer known and it can be made
    assert!(!s.thumbs.is_known_failure(&file));
    assert!(s.thumbs.create(&file, ThumbSize::Normal).is_ok());
}

#[test]
fn a_known_failure_is_reported_as_such_for_the_same_file() {
    let s = setup();
    let file = s.pictures.join("bad.png");
    support::write(&file, b"junk");
    assert!(s.thumbs.create(&file, ThumbSize::Normal).is_err());
    assert!(matches!(
        s.thumbs.create(&file, ThumbSize::Large),
        Err(ThumbError::KnownFailure)
    ));
    assert!(matches!(
        s.thumbs.get_or_create(&file, ThumbSize::Large),
        Err(ThumbError::KnownFailure)
    ));
}

#[test]
fn failure_markers_of_other_applications_are_not_ours() {
    let s = setup();
    let file = s.pictures.join("bad.png");
    support::write(&file, b"junk");
    let other = Thumbnailer::new(&s.cache, "other-app");
    assert!(other.create(&file, ThumbSize::Normal).is_err());
    assert!(other.is_known_failure(&file));
    assert!(!s.thumbs.is_known_failure(&file));
}

#[test]
fn a_directory_or_a_missing_path_is_an_io_error() {
    let s = setup();
    assert!(matches!(
        s.thumbs.create(&s.pictures, ThumbSize::Normal),
        Err(ThumbError::Io(_))
    ));
    assert!(matches!(
        s.thumbs
            .create(&s.pictures.join("none.png"), ThumbSize::Normal),
        Err(ThumbError::Io(_))
    ));
}

#[test]
fn a_picture_wider_than_8192_pixels_is_refused() {
    let s = setup();
    let wide = s.pictures.join("wide.png");
    support::write_png(&wide, 8193, 1, [1, 2, 3, 255]);
    assert!(matches!(
        s.thumbs.create(&wide, ThumbSize::Normal),
        Err(ThumbError::Image(_))
    ));
    let ok = s.pictures.join("ok.png");
    support::write_png(&ok, 8192, 1, [1, 2, 3, 255]);
    assert!(s.thumbs.create(&ok, ThumbSize::Normal).is_ok());
}

#[test]
fn a_file_in_the_cache_is_not_thumbnailed_and_leaves_no_marker() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 300, 300, [1, 2, 3, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    assert!(s.thumbs.is_in_cache(&made));
    assert!(matches!(
        s.thumbs.create(&made, ThumbSize::Normal),
        Err(ThumbError::Io(_))
    ));
    assert!(!s.thumbs.root().join("fail").exists());
}

#[test]
fn a_symbolic_link_into_the_cache_is_in_the_cache() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 300, 300, [1, 2, 3, 255]);
    let made = s.thumbs.create(&picture, ThumbSize::Normal).unwrap();
    let link = s.pictures.join("link.png");
    std::os::unix::fs::symlink(&made, &link).unwrap();
    assert!(s.thumbs.is_in_cache(&link));
    assert!(matches!(
        s.thumbs.create(&link, ThumbSize::Large),
        Err(ThumbError::Io(_))
    ));
    assert!(!s.thumbs.is_in_cache(&picture));
    assert!(!s.thumbs.is_in_cache(&s.pictures.join("none")));
}

#[test]
fn a_pipe_is_not_read() {
    let s = setup();
    let fifo = s.pictures.join("pipe.png");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(matches!(
        s.thumbs.create(&fifo, ThumbSize::Normal),
        Err(ThumbError::Io(_))
    ));
}

#[test]
fn pixels_are_the_cached_or_created_thumbnail_shrunk_to_the_edge() {
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
}

#[test]
fn pixels_are_not_enlarged_past_the_thumbnail() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 600, 300, [1, 2, 3, 255]);
    let px = s.thumbs.pixels(&picture, ThumbSize::Normal, 1000).unwrap();
    assert_eq!((px.width, px.height), (128, 64));
}

#[test]
fn pixels_use_a_larger_cached_thumbnail_before_making_one() {
    let s = setup();
    let picture = s.pictures.join("p.png");
    support::write_png(&picture, 2000, 1000, [1, 2, 3, 255]);
    s.thumbs.create(&picture, ThumbSize::Large).unwrap();
    let px = s.thumbs.pixels(&picture, ThumbSize::Normal, 1000).unwrap();
    assert_eq!((px.width, px.height), (256, 128));
    assert_eq!(
        s.thumbs.lookup(&picture, ThumbSize::Normal),
        None,
        "none made at the smaller size"
    );
}

#[test]
fn pixels_of_a_file_that_is_not_a_picture_are_an_error() {
    let s = setup();
    let file = s.pictures.join("bad.png");
    support::write(&file, b"junk");
    assert!(s.thumbs.pixels(&file, ThumbSize::Normal, 64).is_err());
}

#[test]
fn the_errors_have_messages() {
    assert_eq!(
        ThumbError::KnownFailure.to_string(),
        "previous thumbnail attempt failed"
    );
    let io: ThumbError = std::io::Error::other("boom").into();
    assert_eq!(io.to_string(), "boom");
}
