//! Thumbnail cache following the freedesktop.org Thumbnail Managing Standard.
//!
//! * Location `$XDG_CACHE_HOME/thumbnails/<normal|large|x-large|xx-large>/<md5 of the file URI>.png`
//!   with edge lengths 128, 256, 512 and 1024.
//! * A thumbnail is valid when its `Thumb::MTime` text chunk equals the file's modification time
//!   (and `Thumb::Size`, when present, its byte size).
//! * Created thumbnails carry `Thumb::URI`, `Thumb::MTime`, `Thumb::Size` and `Thumb::Mimetype`;
//!   files are written to a temporary name and renamed, with mode 0600 (directories 0700).
//! * Failures are remembered in `fail/<application>/ so that unreadable files are not retried.
//!
//! Which cache size serves which icon size is approximate: the smallest size not smaller
//! than the icon in device pixels.

use image::imageops::FilterType;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const URI_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b']')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ThumbSize {
    Normal,
    Large,
    XLarge,
    XXLarge,
}

impl ThumbSize {
    pub const ALL: [ThumbSize; 4] = [
        ThumbSize::Normal,
        ThumbSize::Large,
        ThumbSize::XLarge,
        ThumbSize::XXLarge,
    ];
    pub fn edge(self) -> u32 {
        match self {
            ThumbSize::Normal => 128,
            ThumbSize::Large => 256,
            ThumbSize::XLarge => 512,
            ThumbSize::XXLarge => 1024,
        }
    }
    pub fn dir(self) -> &'static str {
        match self {
            ThumbSize::Normal => "normal",
            ThumbSize::Large => "large",
            ThumbSize::XLarge => "x-large",
            ThumbSize::XXLarge => "xx-large",
        }
    }
    /// Smallest cache size covering `icon_points * scale` device pixels.
    pub fn for_icon(icon_points: f64, scale: f64) -> ThumbSize {
        let px = icon_points * scale;
        Self::ALL
            .into_iter()
            .find(|s| s.edge() as f64 >= px)
            .unwrap_or(ThumbSize::XXLarge)
    }
}

/// `file://` URI of an absolute path, percent-encoded as the standard requires.
pub fn file_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    let encoded: Vec<String> = s
        .split('/')
        .map(|seg| utf8_percent_encode(seg, URI_SET).to_string())
        .collect();
    format!("file://{}", encoded.join("/"))
}

pub fn md5_hex(uri: &str) -> String {
    format!("{:x}", md5::compute(uri.as_bytes()))
}

#[derive(Debug)]
pub enum ThumbError {
    Io(io::Error),
    Image(image::ImageError),
    Png(png::EncodingError),
    /// A failure marker exists for this file and modification time.
    KnownFailure,
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThumbError::Io(e) => write!(f, "{e}"),
            ThumbError::Image(e) => write!(f, "{e}"),
            ThumbError::Png(e) => write!(f, "{e}"),
            ThumbError::KnownFailure => write!(f, "previous thumbnail attempt failed"),
        }
    }
}
impl std::error::Error for ThumbError {}
impl From<io::Error> for ThumbError {
    fn from(e: io::Error) -> Self {
        ThumbError::Io(e)
    }
}
impl From<image::ImageError> for ThumbError {
    fn from(e: image::ImageError) -> Self {
        ThumbError::Image(e)
    }
}
impl From<png::EncodingError> for ThumbError {
    fn from(e: png::EncodingError) -> Self {
        ThumbError::Png(e)
    }
}

pub struct Thumbnailer {
    root: PathBuf,
    app: String,
}

fn read_chunks(p: &Path) -> Option<Vec<(String, String)>> {
    let dec = png::Decoder::new(io::BufReader::new(fs::File::open(p).ok()?));
    let reader = dec.read_info().ok()?;
    Some(
        reader
            .info()
            .uncompressed_latin1_text
            .iter()
            .map(|c| (c.keyword.clone(), c.text.clone()))
            .collect(),
    )
}

fn mkdir_private(p: &Path) -> io::Result<()> {
    fs::create_dir_all(p)?;
    let mut perm = fs::metadata(p)?.permissions();
    perm.set_mode(0o700);
    fs::set_permissions(p, perm)
}

impl Thumbnailer {
    /// `cache_home` is `$XDG_CACHE_HOME` (the `thumbnails` directory is created below it).
    pub fn new(cache_home: impl AsRef<Path>, app: &str) -> Self {
        Thumbnailer {
            root: cache_home.as_ref().join("thumbnails"),
            app: app.to_string(),
        }
    }

    pub fn from_env(app: &str) -> Self {
        let home = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(home, app)
    }

    pub fn thumb_path(&self, uri: &str, size: ThumbSize) -> PathBuf {
        self.root
            .join(size.dir())
            .join(format!("{}.png", md5_hex(uri)))
    }

    fn fail_path(&self, uri: &str) -> PathBuf {
        self.root
            .join("fail")
            .join(&self.app)
            .join(format!("{}.png", md5_hex(uri)))
    }

    fn validate(&self, thumb: &Path, mtime: i64, size: u64) -> bool {
        let Some(chunks) = read_chunks(thumb) else {
            return false;
        };
        let get = |k: &str| {
            chunks
                .iter()
                .find(|(kk, _)| kk == k)
                .map(|(_, v)| v.as_str())
        };
        get("Thumb::MTime").and_then(|v| v.parse::<i64>().ok()) == Some(mtime)
            && get("Thumb::Size")
                .map(|v| v.parse::<u64>().ok() == Some(size))
                .unwrap_or(true)
    }

    /// Cached, still valid thumbnail of `path`, if any.
    pub fn lookup(&self, path: &Path, size: ThumbSize) -> Option<PathBuf> {
        let meta = fs::metadata(path).ok()?;
        let uri = file_uri(path);
        let p = self.thumb_path(&uri, size);
        self.validate(&p, meta.mtime(), meta.len()).then_some(p)
    }

    /// Best cached thumbnail not smaller than `want` (falls back to larger sizes first).
    pub fn lookup_at_least(&self, path: &Path, want: ThumbSize) -> Option<(ThumbSize, PathBuf)> {
        ThumbSize::ALL
            .into_iter()
            .filter(|s| *s >= want)
            .find_map(|s| self.lookup(path, s).map(|p| (s, p)))
    }

    pub fn is_known_failure(&self, path: &Path) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        let f = self.fail_path(&file_uri(path));
        read_chunks(&f).and_then(|c| {
            c.into_iter()
                .find(|(k, _)| k == "Thumb::MTime")
                .and_then(|(_, v)| v.parse::<i64>().ok())
        }) == Some(meta.mtime())
    }

    fn write_png(
        &self,
        dest: &Path,
        w: u32,
        h: u32,
        rgba: &[u8],
        meta: &[(&str, String)],
    ) -> Result<(), ThumbError> {
        let dir = dest
            .parent()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        mkdir_private(&self.root)?;
        mkdir_private(dir)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            md5_hex(&dest.to_string_lossy())
        ));
        {
            let file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            let mut enc = png::Encoder::new(io::BufWriter::new(file), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            for (k, v) in meta {
                enc.add_text_chunk((*k).to_string(), v.clone())?;
            }
            let mut wr = enc.write_header()?;
            wr.write_image_data(rgba)?;
        }
        fs::rename(&tmp, dest)?;
        Ok(())
    }

    /// Create the thumbnail of an image file at `size`; returns its path. A failure leaves a marker.
    pub fn create(&self, path: &Path, size: ThumbSize) -> Result<PathBuf, ThumbError> {
        let meta = fs::metadata(path)?;
        let uri = file_uri(path);
        if self.is_known_failure(path) {
            return Err(ThumbError::KnownFailure);
        }
        let decoded = image::ImageReader::open(path)
            .and_then(|r| r.with_guessed_format())
            .map_err(ThumbError::from)
            .and_then(|r| r.decode().map_err(ThumbError::from));
        let img = match decoded {
            Ok(i) => i,
            Err(e) => {
                let chunks = [
                    ("Thumb::URI", uri.clone()),
                    ("Thumb::MTime", meta.mtime().to_string()),
                ];
                let _ = self.write_png(&self.fail_path(&uri), 1, 1, &[0, 0, 0, 0], &chunks);
                return Err(e);
            }
        };
        let (iw, ih) = (img.width(), img.height());
        let edge = size.edge();
        let thumb = if iw <= edge && ih <= edge {
            img.to_rgba8()
        } else {
            img.resize(edge, edge, FilterType::Triangle).to_rgba8()
        };
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        let chunks = [
            ("Thumb::URI", uri.clone()),
            ("Thumb::MTime", meta.mtime().to_string()),
            ("Thumb::Size", meta.len().to_string()),
            ("Thumb::Mimetype", mime),
            ("Thumb::Image::Width", iw.to_string()),
            ("Thumb::Image::Height", ih.to_string()),
        ];
        let dest = self.thumb_path(&uri, size);
        self.write_png(
            &dest,
            thumb.width(),
            thumb.height(),
            thumb.as_raw(),
            &chunks,
        )?;
        Ok(dest)
    }

    pub fn get_or_create(&self, path: &Path, size: ThumbSize) -> Result<PathBuf, ThumbError> {
        match self.lookup(path, size) {
            Some(p) => Ok(p),
            None => self.create(path, size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_image(path: &Path, w: u32, h: u32) {
        let mut img = image::RgbaImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
        img.save(path).unwrap();
    }

    #[test]
    fn sizes_and_uri() {
        assert_eq!(ThumbSize::Normal.edge(), 128);
        assert_eq!(ThumbSize::XXLarge.edge(), 1024);
        assert_eq!(ThumbSize::for_icon(64.0, 1.0), ThumbSize::Normal);
        assert_eq!(ThumbSize::for_icon(64.0, 2.0), ThumbSize::Normal);
        assert_eq!(ThumbSize::for_icon(128.0, 2.0), ThumbSize::Large);
        assert_eq!(ThumbSize::for_icon(512.0, 2.0), ThumbSize::XXLarge);
        assert_eq!(
            file_uri(Path::new("/home/u/a b/é.png")),
            "file:///home/u/a%20b/%C3%A9.png"
        );
        // value computed with coreutils `md5sum` for the example URI of the standard
        assert_eq!(
            md5_hex("file:///home/jens/photo%20(real%20nice).jpg"),
            "a79ec3befecdc323333753d2124942ba"
        );
        assert_eq!(
            file_uri(Path::new("/home/jens/photo (real nice).jpg")),
            "file:///home/jens/photo%20(real%20nice).jpg"
        );
    }

    #[test]
    fn create_lookup_and_invalidate() {
        let d = tempfile::tempdir().unwrap();
        let t = Thumbnailer::new(d.path().join("cache"), "cupertino-files");
        let img = d.path().join("p.png");
        write_image(&img, 400, 200);
        assert!(t.lookup(&img, ThumbSize::Normal).is_none());
        let p = t.get_or_create(&img, ThumbSize::Normal).unwrap();
        let decoded = image::open(&p).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (128, 64));
        assert_eq!(t.lookup(&img, ThumbSize::Normal), Some(p.clone()));
        let chunks = read_chunks(&p).unwrap();
        assert!(chunks
            .iter()
            .any(|(k, v)| k == "Thumb::URI" && v.starts_with("file://")));
        assert!(chunks
            .iter()
            .any(|(k, v)| k == "Thumb::Mimetype" && v == "image/png"));
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // modifying the file invalidates the thumbnail
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_image(&img, 300, 300);
        assert!(t.lookup(&img, ThumbSize::Normal).is_none());
        assert_eq!(t.lookup_at_least(&img, ThumbSize::Normal), None);
        t.get_or_create(&img, ThumbSize::Large).unwrap();
        assert_eq!(
            t.lookup_at_least(&img, ThumbSize::Normal).map(|x| x.0),
            Some(ThumbSize::Large)
        );
    }

    #[test]
    fn small_images_are_not_upscaled() {
        let d = tempfile::tempdir().unwrap();
        let t = Thumbnailer::new(d.path(), "x");
        let img = d.path().join("s.png");
        write_image(&img, 20, 10);
        let p = t.create(&img, ThumbSize::Normal).unwrap();
        let decoded = image::open(&p).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (20, 10));
    }

    #[test]
    fn failures_are_remembered() {
        let d = tempfile::tempdir().unwrap();
        let t = Thumbnailer::new(d.path(), "x");
        let bad = d.path().join("bad.png");
        fs::write(&bad, b"not an image").unwrap();
        assert!(t.create(&bad, ThumbSize::Normal).is_err());
        assert!(t.is_known_failure(&bad));
        assert!(matches!(
            t.create(&bad, ThumbSize::Normal),
            Err(ThumbError::KnownFailure)
        ));
    }
}
