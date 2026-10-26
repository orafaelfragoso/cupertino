//! PDF rasterisation through poppler's command line tools.
//!
//! `pdfinfo` reports the page size in points and `pdftocairo -png -transp -scale-to-x W
//! -scale-to-y H` renders the page with a transparent background at an exact pixel size. (The
//! plain `pdftoppm` tool cannot produce transparency, so it is not used.) A page that stacks
//! animation frames vertically is rendered as one image and cut into frames by the caller.

use std::path::{Path, PathBuf};
use std::process::Command;
use tiny_skia::Pixmap;

#[derive(Clone, Debug)]
pub struct Poppler {
    pub pdftocairo: PathBuf,
    pub pdfinfo: PathBuf,
}

impl Default for Poppler {
    fn default() -> Self {
        Self {
            pdftocairo: "pdftocairo".into(),
            pdfinfo: "pdfinfo".into(),
        }
    }
}

impl Poppler {
    /// `true` when both tools can be run.
    pub fn available(&self) -> bool {
        [&self.pdftocairo, &self.pdfinfo]
            .iter()
            .all(|t| Command::new(t).arg("-v").output().is_ok())
    }

    /// Page size in points.
    pub fn page_size(&self, pdf: &Path) -> Result<(f64, f64), String> {
        let out = Command::new(&self.pdfinfo)
            .arg(pdf)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.pdfinfo.display()))?;
        if !out.status.success() {
            return Err(format!(
                "pdfinfo failed for {}: {}",
                pdf.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        parse_page_size(&String::from_utf8_lossy(&out.stdout))
            .ok_or_else(|| format!("no page size in the pdfinfo output for {}", pdf.display()))
    }

    /// Render page 1 at exactly `w × h` pixels with a transparent background.
    pub fn render(&self, pdf: &Path, w: u32, h: u32) -> Result<Pixmap, String> {
        let dir = std::env::temp_dir().join(format!(
            "cupertino-cursor-{}-{}",
            std::process::id(),
            unique()
        ));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let base = dir.join("page");
        let out = Command::new(&self.pdftocairo)
            .args([
                "-png",
                "-transp",
                "-singlefile",
                "-f",
                "1",
                "-l",
                "1",
                "-scale-to-x",
                &w.to_string(),
                "-scale-to-y",
                &h.to_string(),
            ])
            .arg(pdf)
            .arg(&base)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.pdftocairo.display()));
        let result = out.and_then(|o| {
            if !o.status.success() {
                return Err(format!(
                    "pdftocairo failed for {}: {}",
                    pdf.display(),
                    String::from_utf8_lossy(&o.stderr).trim()
                ));
            }
            Pixmap::load_png(dir.join("page.png"))
                .map_err(|e| format!("cannot read the rendered page: {e}"))
        });
        let _ = std::fs::remove_dir_all(&dir);
        let pm = result?;
        if (pm.width(), pm.height()) != (w, h) {
            return Err(format!(
                "expected {w}x{h} pixels from {}, got {}x{}",
                pdf.display(),
                pm.width(),
                pm.height()
            ));
        }
        Ok(pm)
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// Extract `W x H` from the `Page size:` line of `pdfinfo` output.
pub fn parse_page_size(text: &str) -> Option<(f64, f64)> {
    let line = text.lines().find(|l| l.starts_with("Page size:"))?;
    let rest = line.trim_start_matches("Page size:").trim();
    let mut it = rest.split_whitespace();
    let w: f64 = it.next()?.parse().ok()?;
    if it.next()? != "x" {
        return None;
    }
    let h: f64 = it.next()?.parse().ok()?;
    (w > 0.0 && h > 0.0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_size_parsing() {
        assert_eq!(
            parse_page_size("Title: x\nPages:           1\nPage size:       28 x 600 pts\n"),
            Some((28.0, 600.0))
        );
        assert_eq!(
            parse_page_size("Page size:   612.5 x 792 pts (letter)"),
            Some((612.5, 792.0))
        );
        assert_eq!(parse_page_size("Pages: 1"), None);
        assert_eq!(parse_page_size("Page size: a x b"), None);
        assert_eq!(parse_page_size("Page size: 0 x 10 pts"), None);
    }

    #[test]
    fn missing_tools_are_reported() {
        let p = Poppler {
            pdftocairo: "/nonexistent/pdftocairo".into(),
            pdfinfo: "/nonexistent/pdfinfo".into(),
        };
        assert!(!p.available());
        assert!(p
            .page_size(Path::new("x.pdf"))
            .unwrap_err()
            .contains("cannot run"));
        assert!(p
            .render(Path::new("x.pdf"), 4, 4)
            .unwrap_err()
            .contains("cannot run"));
    }
}
