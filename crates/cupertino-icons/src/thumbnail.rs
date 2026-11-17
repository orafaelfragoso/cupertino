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

/// The largest file, and the largest side and memory of a picture, that is decoded for a thumbnail.
const MAX_FILE_BYTES: u64 = 256 << 20;
const MAX_SIDE: u32 = 8_192;
const MAX_ALLOC: u64 = 256 << 20;

/// The file at `path`, opened without blocking (a pipe or a device put there would not hang the caller), checked to be
/// a regular file through the descriptor itself and not too large, then switched to blocking reads for the decoder.
fn open_checked(path: &Path) -> Result<fs::File, ThumbError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(ThumbError::Io(io::Error::other("not a regular file")));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(ThumbError::Io(io::Error::other("the file is too large")));
    }
    let flags = rustix::fs::fcntl_getfl(&file).map_err(io::Error::from)?;
    rustix::fs::fcntl_setfl(&file, flags - rustix::fs::OFlags::NONBLOCK)
        .map_err(io::Error::from)?;
    Ok(file)
}

#[derive(Clone, Debug)]
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

    /// The `thumbnails` directory of the cache. The standard has no thumbnails made of the files under it.
    pub fn root(&self) -> &Path {
        &self.root
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
        // one name per write: two threads of a process making the same thumbnail never share a temporary file
        static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!(
            ".tmp-{}-{write}-{}",
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

    /// Whether `path` is under the cache directory, by its canonical path: the standard makes no thumbnails of
    /// thumbnails, also when a symbolic link leads into the cache.
    pub fn is_in_cache(&self, path: &Path) -> bool {
        if path.starts_with(&self.root) {
            return true;
        }
        match (fs::canonicalize(path), fs::canonicalize(&self.root)) {
            (Ok(path), Ok(root)) => path.starts_with(root),
            _ => false,
        }
    }

    /// Create the thumbnail of an image file at `size`; returns its path. A failure leaves a marker. A file under the
    /// cache directory itself is refused without a marker.
    pub fn create(&self, path: &Path, size: ThumbSize) -> Result<PathBuf, ThumbError> {
        let meta = fs::metadata(path)?;
        if self.is_in_cache(path) {
            return Err(ThumbError::Io(io::Error::other(
                "the file is in the thumbnail cache",
            )));
        }
        let uri = file_uri(path);
        if self.is_known_failure(path) {
            return Err(ThumbError::KnownFailure);
        }
        let decoded = open_checked(path)
            .and_then(|file| {
                let mut reader =
                    image::ImageReader::new(io::BufReader::new(file)).with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_image_width = Some(MAX_SIDE);
                limits.max_image_height = Some(MAX_SIDE);
                limits.max_alloc = Some(MAX_ALLOC);
                reader.limits(limits);
                let mime = reader.format().map(|f| f.to_mime_type());
                Ok((reader, mime))
            })
            .and_then(|(reader, mime)| {
                reader
                    .decode()
                    .map(|img| (img, mime))
                    .map_err(ThumbError::from)
            });
        let (img, mime) = match decoded {
            Ok(decoded) => decoded,
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
        let mime = mime.unwrap_or("application/octet-stream").to_string();
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

    /// The thumbnail of `path` as straight RGBA pixels at most `edge` pixels a side: a valid cached one of `size` or
    /// larger, else one created at `size`, shrunk to fit `edge` with its aspect ratio kept and never enlarged.
    pub fn pixels(&self, path: &Path, size: ThumbSize, edge: u32) -> Result<Pixels, ThumbError> {
        let thumb = match self.lookup_at_least(path, size) {
            Some((_, p)) => p,
            None => self.create(path, size)?,
        };
        let mut reader = image::ImageReader::open(&thumb)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(ThumbSize::XXLarge.edge());
        limits.max_image_height = Some(ThumbSize::XXLarge.edge());
        reader.limits(limits);
        let img = reader.decode()?;
        let edge = edge.max(1);
        let img = if img.width() > edge || img.height() > edge {
            img.resize(edge, edge, FilterType::Triangle)
        } else {
            img
        };
        let rgba = img.to_rgba8();
        Ok(Pixels {
            width: rgba.width(),
            height: rgba.height(),
            rgba: rgba.into_raw(),
        })
    }
}

/// A decoded thumbnail: straight (not premultiplied) RGBA, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
