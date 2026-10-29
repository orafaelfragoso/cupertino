//! What the machine must offer before the TTY backend takes the display: a report of the primary GPU's driver and the
//! settings a driver needs, with the fix for each one that is wrong. Read-only: it opens no device.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// Where GBM looks for the driver-supplied buffer backends.
const GBM_DIRS: [&str; 3] = [
    "/usr/lib/gbm",
    "/usr/lib64/gbm",
    "/usr/lib/x86_64-linux-gnu/gbm",
];
const NVIDIA_GBM_BACKEND: &str = "nvidia-drm_gbm.so";

#[derive(Debug, PartialEq, Eq)]
pub struct Finding {
    pub what: String,
    pub fix: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.what, self.fix)
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The kernel driver bound to the primary GPU, if sysfs names one.
    pub driver: Option<String>,
    /// Facts worth having in the log.
    pub notes: Vec<String>,
    /// The start would fail or misbehave.
    pub problems: Vec<Finding>,
    /// The start may work, but this is likely to cause trouble.
    pub warnings: Vec<Finding>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.problems.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for note in &self.notes {
            writeln!(f, "note: {note}")?;
        }
        for warning in &self.warnings {
            writeln!(f, "warning: {warning}")?;
        }
        for problem in &self.problems {
            writeln!(f, "problem: {problem}")?;
        }
        Ok(())
    }
}

/// Checks the primary GPU of `sysfs_root` (`/sys` in production).
pub fn run(sysfs_root: &Path) -> Result<Report> {
    let gpu = cupertino_devices::primary_gpu(sysfs_root)
        .map_err(|e| anyhow!("listing GPUs: {e}"))?
        .context("no GPU found: /sys/class/drm lists no card")?;
    let gbm_dirs: Vec<PathBuf> = GBM_DIRS.iter().map(PathBuf::from).collect();
    Ok(check(sysfs_root, gpu.device.syspath(), &gbm_dirs))
}

fn check(sysfs_root: &Path, card: &Path, gbm_dirs: &[PathBuf]) -> Report {
    let mut report = Report::default();
    let name = card
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    report.driver = std::fs::read_link(card.join("device/driver"))
        .ok()
        .and_then(|link| link.file_name().map(|n| n.to_string_lossy().into_owned()));
    match &report.driver {
        Some(driver) => report
            .notes
            .push(format!("primary GPU {name} uses the {driver} driver")),
        None => report.warnings.push(Finding {
            what: format!("primary GPU {name} has no driver bound"),
            fix: "load the GPU's kernel driver (check `lspci -k` and `dmesg`)".into(),
        }),
    }
    if report.driver.as_deref() == Some("nvidia") {
        check_nvidia(sysfs_root, gbm_dirs, &mut report);
    }
    report
}

fn module_attr(sysfs_root: &Path, module: &str, attr: &str) -> Option<String> {
    let path = sysfs_root.join("module").join(module).join(attr);
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn check_nvidia(sysfs_root: &Path, gbm_dirs: &[PathBuf], report: &mut Report) {
    if let Some(version) = module_attr(sysfs_root, "nvidia", "version") {
        report.notes.push(format!("nvidia driver {version}"));
    }
    match module_attr(sysfs_root, "nvidia_drm", "parameters/modeset") {
        None => report.problems.push(Finding {
            what: "the nvidia_drm kernel module is not loaded".into(),
            fix:
                "load it with `modprobe nvidia_drm modeset=1` and set the option in /etc/modprobe.d"
                    .into(),
        }),
        Some(value) if value != "Y" => report.problems.push(Finding {
            what: "nvidia-drm modesetting is off, so the driver cannot drive a display".into(),
            fix: "add `nvidia-drm.modeset=1` to the kernel command line and reboot".into(),
        }),
        Some(_) => {}
    }
    if let Some(value) = module_attr(sysfs_root, "nvidia_drm", "parameters/fbdev") {
        report
            .notes
            .push(format!("nvidia-drm fbdev is {}", flag(&value)));
    }
    if !gbm_dirs
        .iter()
        .any(|dir| dir.join(NVIDIA_GBM_BACKEND).exists())
    {
        report.warnings.push(Finding {
            what: format!("no {NVIDIA_GBM_BACKEND} GBM backend was found"),
            fix: "install the driver's GBM library (nvidia-utils); without it buffer allocation fails".into(),
        });
    }
}

fn flag(value: &str) -> &str {
    match value {
        "Y" | "1" => "on",
        "N" | "0" => "off",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    struct Machine {
        root: tempfile::TempDir,
    }

    impl Machine {
        fn with_driver(driver: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let card = root.path().join("devices/card1");
            fs::create_dir_all(card.join("device")).unwrap();
            fs::create_dir_all(root.path().join("bus/pci/drivers").join(driver)).unwrap();
            symlink(
                root.path().join("bus/pci/drivers").join(driver),
                card.join("device/driver"),
            )
            .unwrap();
            Machine { root }
        }

        fn card(&self) -> PathBuf {
            self.root.path().join("devices/card1")
        }

        fn module(&self, module: &str, attr: &str, value: &str) {
            let path = self.root.path().join("module").join(module).join(attr);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, format!("{value}\n")).unwrap();
        }

        fn gbm_dir(&self, with_nvidia: bool) -> Vec<PathBuf> {
            let dir = self.root.path().join("gbm");
            fs::create_dir_all(&dir).unwrap();
            if with_nvidia {
                fs::write(dir.join(NVIDIA_GBM_BACKEND), "").unwrap();
            }
            vec![dir]
        }

        fn check(&self, gbm: &[PathBuf]) -> Report {
            check(self.root.path(), &self.card(), gbm)
        }
    }

    #[test]
    fn a_driver_that_needs_nothing_is_reported_and_passes() {
        let m = Machine::with_driver("amdgpu");
        let report = m.check(&[]);
        assert_eq!(report.driver.as_deref(), Some("amdgpu"));
        assert!(report.is_ok() && report.warnings.is_empty());
    }

    #[test]
    fn a_gpu_without_a_bound_driver_is_a_warning() {
        let m = Machine::with_driver("amdgpu");
        fs::remove_file(m.card().join("device/driver")).unwrap();
        let report = m.check(&[]);
        assert!(report.is_ok());
        assert_eq!(report.driver, None);
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn nvidia_with_modesetting_on_and_the_gbm_backend_passes() {
        let m = Machine::with_driver("nvidia");
        m.module("nvidia", "version", "615.78.08");
        m.module("nvidia_drm", "parameters/modeset", "Y");
        m.module("nvidia_drm", "parameters/fbdev", "Y");
        let report = m.check(&m.gbm_dir(true));
        assert!(report.is_ok() && report.warnings.is_empty(), "{report}");
        assert!(report.notes.iter().any(|n| n == "nvidia driver 615.78.08"));
    }

    #[test]
    fn nvidia_with_modesetting_off_names_the_kernel_option() {
        let m = Machine::with_driver("nvidia");
        m.module("nvidia_drm", "parameters/modeset", "N");
        let report = m.check(&m.gbm_dir(true));
        assert_eq!(report.problems.len(), 1);
        assert!(report.problems[0].fix.contains("nvidia-drm.modeset=1"));
    }

    #[test]
    fn nvidia_without_the_drm_module_is_a_problem_and_without_gbm_a_warning() {
        let m = Machine::with_driver("nvidia");
        let report = m.check(&m.gbm_dir(false));
        assert_eq!(report.problems.len(), 1);
        assert!(report.problems[0].what.contains("not loaded"));
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].what.contains(NVIDIA_GBM_BACKEND));
    }
}
