//! Small cargo workspaces written into a temporary directory, checked through the library or the binary.
//!
//! A workspace has its crates in `crates/<name>`, each declaring its layer as real crates do. A dependency on a name
//! that is not one of its crates is an external crate: an empty path crate beside the workspace, outside it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use cupertino_layers::{
    check_workspace_with, clippy_toml, Findings, Layer, Policy, DISALLOWED_METHODS, POLICY,
};

struct Krate {
    name: String,
    layer: Layer,
    licence: Option<String>,
    deps: Vec<(String, String)>,
    files: Vec<(String, String)>,
    clippy: bool,
}

pub struct Fixture {
    base: PathBuf,
    root: PathBuf,
    crates: Vec<Krate>,
    remaining: Option<String>,
}

impl Fixture {
    pub fn new(name: &str) -> Fixture {
        let base = std::env::temp_dir().join(format!(
            "cupertino-layers-tests-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        Fixture {
            root: base.join("ws"),
            base,
            crates: Vec::new(),
            remaining: None,
        }
    }

    /// A crate of `layer` with MIT as its licence, no sources, and, in a `shell` or `apps` crate, the generated
    /// `clippy.toml`. `deps` are `(kind, name)` with kind `normal`, `dev` or `build`.
    pub fn krate(mut self, name: &str, layer: Layer, deps: &[(&str, &str)]) -> Fixture {
        self.crates.push(Krate {
            name: name.to_owned(),
            layer,
            licence: Some("MIT".to_owned()),
            deps: deps
                .iter()
                .map(|(k, n)| ((*k).to_owned(), (*n).to_owned()))
                .collect(),
            files: Vec::new(),
            clippy: matches!(layer, Layer::Shell | Layer::Apps),
        });
        self
    }

    fn named(&mut self, name: &str) -> &mut Krate {
        self.crates
            .iter_mut()
            .find(|k| k.name == name)
            .unwrap_or_else(|| panic!("no crate {name}"))
    }

    /// A file of crate `name`, by path relative to the crate.
    pub fn file(mut self, name: &str, path: &str, content: &str) -> Fixture {
        self.named(name)
            .files
            .push((path.to_owned(), content.to_owned()));
        self
    }

    /// The crate's `license`; `None` leaves it out. `file:<x>` declares a `license-file` instead.
    pub fn licence(mut self, name: &str, licence: Option<&str>) -> Fixture {
        self.named(name).licence = licence.map(str::to_owned);
        self
    }

    /// The crate has no `clippy.toml`.
    pub fn no_clippy(mut self, name: &str) -> Fixture {
        self.named(name).clippy = false;
        self
    }

    /// The remaining-work file of the workspace, lines without header.
    pub fn remaining(mut self, lines: &[&str]) -> Fixture {
        self.remaining = Some(lines.iter().map(|l| format!("{l}\n")).collect());
        self
    }

    /// Writes the workspace, with its lock file, and returns it.
    pub fn build(self) -> Fixture {
        let write = |path: PathBuf, content: &str| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        };
        let members: Vec<&str> = self.crates.iter().map(|k| k.name.as_str()).collect();
        let mut manifest = String::from("[workspace]\nresolver = \"2\"\nmembers = [");
        for m in &members {
            manifest.push_str(&format!("\"crates/{m}\", "));
        }
        manifest.push_str("]\n");
        write(self.root.join("Cargo.toml"), &manifest);
        let mut externals = Vec::new();
        for k in &self.crates {
            let dir = self.root.join("crates").join(&k.name);
            let mut m = format!(
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                k.name
            );
            match k.licence.as_deref() {
                Some(l) if l.starts_with("file:") => {
                    m.push_str("license-file = \"LICENSE\"\n");
                    write(dir.join("LICENSE"), "terms\n");
                }
                Some(l) => m.push_str(&format!("license = \"{l}\"\n")),
                None => {}
            }
            m.push_str(&format!(
                "[package.metadata.cupertino]\nlayer = \"{}\"\n",
                k.layer.name()
            ));
            for (kind, section) in [
                ("normal", "dependencies"),
                ("dev", "dev-dependencies"),
                ("build", "build-dependencies"),
            ] {
                let of_kind: Vec<_> = k.deps.iter().filter(|(d, _)| d == kind).collect();
                if of_kind.is_empty() {
                    continue;
                }
                m.push_str(&format!("[{section}]\n"));
                for (_, to) in of_kind {
                    let path = if members.contains(&to.as_str()) {
                        format!("../{to}")
                    } else {
                        externals.push(to.clone());
                        format!("{}ext/{to}", "../".repeat(3))
                    };
                    m.push_str(&format!("{to} = {{ path = \"{path}\" }}\n"));
                }
            }
            write(dir.join("Cargo.toml"), &m);
            write(dir.join("src/lib.rs"), "");
            for (path, content) in &k.files {
                write(dir.join(path), content);
            }
            if k.clippy {
                write(dir.join("clippy.toml"), &clippy_toml(DISALLOWED_METHODS));
            }
        }
        externals.sort();
        externals.dedup();
        for name in externals {
            let dir = self.base.join("ext").join(&name);
            write(
                dir.join("Cargo.toml"),
                &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
            );
            write(dir.join("src/lib.rs"), "");
        }
        if let Some(lines) = &self.remaining {
            write(self.root.join(cupertino_layers::REMAINING), lines);
        }
        let out = cargo()
            .args(["generate-lockfile", "--offline"])
            .current_dir(&self.root)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        self
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    pub fn check(&self) -> Findings {
        self.check_with(&POLICY)
    }

    pub fn check_with(&self, policy: &Policy) -> Findings {
        check_workspace_with(&self.root, &cargo, policy).expect("the fixture is a workspace")
    }

    /// The remaining-work lines of every problem of the rules, sorted.
    pub fn lines(&self) -> Vec<String> {
        self.check()
            .problems
            .iter()
            .filter_map(|p| p.line())
            .collect()
    }

    /// Runs the binary in the workspace.
    pub fn run(&self, args: &[&str]) -> Output {
        run_in(&self.root, args)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

pub fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cupertino-layers"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}
