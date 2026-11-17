//! The licence checks of the gate: the package recipes install licence texts that exist, declare every licence of
//! what they ship, the SDK carries the text of each licence it links, and the third-party list and notices match the
//! crates the workspace depends on.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::plan;

/// The platform the packages are built for; dependencies of other platforms do not ship.
const TARGET: &str = "x86_64-unknown-linux-gnu";
/// The directory of the package recipes, one `<recipe>/PKGBUILD` each.
const RECIPES: &str = "packaging/arch";
/// The registry crates the workspace depends on and their licence texts, written by `REGENERATE`.
const THIRD_PARTY: &str = "packaging/licenses/THIRD-PARTY-LICENSES";
/// The NOTICE files of those crates, the notices of the files they compile in and the licence texts only those notices
/// name, kept by hand.
const NOTICES: &str = "packaging/licenses/THIRD-PARTY-NOTICES";
/// The command that writes `THIRD_PARTY`.
const REGENERATE: &str = "cargo about generate --workspace --all-features --locked \
    -c packaging/licenses/about.toml packaging/licenses/about.hbs -o packaging/licenses/THIRD-PARTY-LICENSES";
/// What the data package ships besides crate code: the pack's own files, the font and the photographs.
const DATA_LICENSES: [&str; 3] = ["GPL-3.0-or-later", "OFL-1.1", "LicenseRef-Unsplash"];
/// The file name a licence text is installed under when it is not the SPDX identifier.
const TEXT_NAMES: [(&str, &str); 1] = [("OFL-1.1", "OFL.txt")];
/// Registry crates that carry licensed input without a licence file: a build script compiles bindings generated from
/// these files, whose leading comment holds the licence and copyright notice.
const EMBEDDED_NOTICES: [(&str, &[&str]); 1] =
    [("khronos_api", &["api_egl/api/egl.xml", "api/xml/gl.xml"])];
/// Crates whose protocol XML files wayland-scanner compiles in: every `"....xml"` literal of their sources is an input
/// of the scanner's macros, and each file's `<copyright>` holds its licence notice.
const SCANNER_CRATES: [&str; 6] = [
    "wayland-client",
    "wayland-server",
    "wayland-protocols",
    "wayland-protocols-wlr",
    "wayland-protocols-plasma",
    "wayland-protocols-misc",
];

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).expect("licence patterns are valid")
}

static FUNCTION: LazyLock<Regex> = LazyLock::new(|| regex(r"^package_([A-Za-z0-9._+-]+)\(\) \{$"));
static LIST: LazyLock<Regex> = LazyLock::new(|| regex(r"^license=\((.*)\)$"));
static QUOTED: LazyLock<Regex> = LazyLock::new(|| regex(r#"'([^']*)'|"([^"]*)""#));
static LICENCE_DIRECTORY: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"\blic="\$pkgdir/usr/share/licenses/([^"/$]+)""#));
static LICENCE_INSTALL: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^install -Dm644 ([^\s"'$*?{}\\]+) "\$lic/([^"/$]+)"$"#));
static BINARIES: LazyLock<Regex> = LazyLock::new(|| regex(r"^for name in ([^;]+); do$"));
static LIBRARY: LazyLock<Regex> =
    LazyLock::new(|| regex(r#"^install -Dm755 "[^"]*/release/lib([A-Za-z0-9_]+)\.so" "#));
static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| regex(r"[\w.+-]+"));
static OPERATOR: LazyLock<Regex> = LazyLock::new(|| regex(r"\b(?:AND|OR|WITH)\b"));
static SPDX_LINE: LazyLock<Regex> =
    LazyLock::new(|| regex(r"SPDX-License-Identifier:[ \t]*([^\n]+)"));
static NOTICE_FILE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?i)^NOTICES?(?:\..+)?$"));
static XML_LITERAL: LazyLock<Regex> = LazyLock::new(|| regex(r#""([^"\s]+\.xml)""#));
static REGISTRY_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| regex(r"(?s)<registry>\s*(?:<!--(.*?)-->|<comment>(.*?)</comment>)"));
static COPYRIGHT: LazyLock<Regex> = LazyLock::new(|| regex(r"(?s)<copyright>(.*?)</copyright>"));
static ENTITY: LazyLock<Regex> = LazyLock::new(|| regex(r"&(#[0-9]+|#[xX][0-9a-fA-F]+|[a-z]+);"));
static NOTICE_HEADING: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^--- .*\(([^()\n]+)\)$"));

static TREE_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\d+)(\S+) v(\S+)"));

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    source: Option<String>,
    license: Option<String>,
    license_file: Option<String>,
    manifest_path: PathBuf,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
}

impl Package {
    fn label(&self) -> String {
        format!("{} {}", self.name, self.version)
    }

    fn directory(&self) -> &Path {
        self.manifest_path.parent().unwrap_or(Path::new("."))
    }

    fn workspace(&self) -> bool {
        self.source.is_none()
    }
}

/// The SPDX identifiers an expression names.
fn identifiers(expression: &str) -> BTreeSet<String> {
    let bare = OPERATOR.replace_all(expression, " ");
    IDENTIFIER
        .find_iter(&bare)
        .map(|found| found.as_str().to_owned())
        .collect()
}

/// The dependency graph of the workspace as the packages build it (`--workspace --all-features` for the packages'
/// platform): the packages from `cargo metadata`, and the normal and build dependencies between them from
/// `cargo tree`, whose feature resolution is the build's (a generator run at build time compiles its output into the
/// binaries). Packages are keyed by `name version`.
struct Graph {
    metadata: Metadata,
    packages: HashMap<String, usize>,
    edges: HashMap<String, BTreeSet<String>>,
    roots: BTreeSet<String>,
}

impl Graph {
    fn read(root: &Path) -> Result<Self, String> {
        let text = plan::output(
            root,
            "cargo",
            &[
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--all-features",
            ],
        )?;
        let metadata: Metadata =
            serde_json::from_str(&text).map_err(|error| format!("cargo metadata: {error}"))?;
        let packages: HashMap<String, usize> = metadata
            .packages
            .iter()
            .enumerate()
            .map(|(index, package)| (package.label(), index))
            .collect();
        // Deduplicated, a package's dependencies are listed under its first appearance only, which is all the edges
        // need.
        let tree = plan::output(
            root,
            "cargo",
            &[
                "tree",
                "--locked",
                "--workspace",
                "--all-features",
                "--target",
                TARGET,
                "--edges",
                "normal,build",
                "--prefix",
                "depth",
                "--format",
                "{p}",
            ],
        )?;
        let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut roots = BTreeSet::new();
        let mut parents: Vec<String> = Vec::new();
        for line in tree.lines().filter(|line| !line.is_empty()) {
            let found = TREE_LINE
                .captures(line)
                .ok_or_else(|| format!("cargo tree: cannot read `{line}`"))?;
            let depth: usize = found[1]
                .parse()
                .map_err(|_| format!("cargo tree: cannot read `{line}`"))?;
            let label = format!("{} {}", &found[2], &found[3]);
            if !packages.contains_key(&label) {
                return Err(format!("cargo tree: {label} is not in cargo metadata"));
            }
            parents.truncate(depth);
            if parents.len() != depth {
                return Err(format!("cargo tree: `{line}` skips a level"));
            }
            match parents.last() {
                Some(parent) => {
                    edges
                        .entry(parent.clone())
                        .or_default()
                        .insert(label.clone());
                }
                None => {
                    roots.insert(label.clone());
                }
            }
            parents.push(label);
        }
        Ok(Self {
            metadata,
            packages,
            edges,
            roots,
        })
    }

    fn package(&self, label: &str) -> Option<&Package> {
        self.packages
            .get(label)
            .map(|index| &self.metadata.packages[*index])
    }

    /// The packages in the closure of `roots` (`name version`), sorted by that.
    fn closure<'g>(&'g self, roots: &BTreeSet<String>) -> Vec<&'g Package> {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut queue: Vec<&str> = roots.iter().map(String::as_str).collect();
        while let Some(label) = queue.pop() {
            if !seen.insert(label) {
                continue;
            }
            if let Some(next) = self.edges.get(label) {
                queue.extend(next.iter().map(String::as_str));
            }
        }
        seen.into_iter()
            .filter_map(|label| self.package(label))
            .collect()
    }

    /// The workspace member that builds a target of `kind` named `name`.
    fn member_with(&self, kind: &str, name: &str) -> Option<&Package> {
        self.metadata.packages.iter().find(|package| {
            self.metadata.workspace_members.contains(&package.id)
                && package.targets.iter().any(|target| {
                    target.name == name && target.kind.iter().any(|found| found == kind)
                })
        })
    }
}

/// A package function of a recipe: `package_<name>() { ... }`, its lines joined across continuations.
struct Function {
    name: String,
    license: Option<BTreeSet<String>>,
    lines: Vec<String>,
}

/// A `PKGBUILD`: its top-level `license=()` and its package functions.
struct Recipe {
    path: String,
    license: Option<BTreeSet<String>>,
    functions: Vec<Function>,
}

/// What a package function ships.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Runtime,
    Data,
    Sdk,
}

impl Kind {
    fn of(name: &str) -> Option<Self> {
        match name.strip_suffix("-git").unwrap_or(name) {
            "cupertino" => Some(Self::Runtime),
            "cupertino-data" => Some(Self::Data),
            "cupertino-kit" => Some(Self::Sdk),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Data => "data",
            Self::Sdk => "sdk",
        }
    }
}

/// The items of a `license=(...)` line, or `None` when anything but quoted items is inside.
fn license_list(line: &str) -> Option<BTreeSet<String>> {
    let inner = LIST.captures(line)?.get(1)?.as_str();
    if !QUOTED.replace_all(inner, "").trim().is_empty() {
        return None;
    }
    Some(
        QUOTED
            .captures_iter(inner)
            .filter_map(|found| found.get(1).or_else(|| found.get(2)))
            .map(|found| found.as_str().to_owned())
            .collect(),
    )
}

/// The lines of `text` with backslash continuations joined (a continuation without its indentation), each with its
/// first line number.
fn joined_lines(text: &str) -> Vec<(usize, String)> {
    let mut lines = Vec::new();
    let mut pending: Option<(usize, String)> = None;
    for (index, line) in text.lines().enumerate() {
        let (start, mut joined, line) = match pending.take() {
            Some((start, joined)) => (start, joined, line.trim_start()),
            None => (index + 1, String::new(), line),
        };
        if let Some(head) = line.strip_suffix('\\') {
            joined.push_str(head);
            pending = Some((start, joined));
        } else {
            joined.push_str(line);
            lines.push((start, joined));
        }
    }
    lines.extend(pending);
    lines
}

fn read_recipe(path: &str, text: &str, problems: &mut Vec<String>) -> Recipe {
    let mut recipe = Recipe {
        path: path.to_owned(),
        license: None,
        functions: Vec::new(),
    };
    let mut current: Option<Function> = None;
    for (number, line) in joined_lines(text) {
        if current.is_some() && line == "}" {
            recipe.functions.extend(current.take());
            continue;
        }
        if let Some(function) = current.as_mut() {
            let trimmed = line.trim();
            if trimmed.starts_with("license=") {
                function.license = license_list(trimmed);
                if function.license.is_none() {
                    problems.push(format!(
                        "{path}:{number}: cannot read the license=() of package_{}()",
                        function.name
                    ));
                }
            }
            function.lines.push(trimmed.to_owned());
        } else if let Some(found) = FUNCTION.captures(&line) {
            current = Some(Function {
                name: found[1].to_owned(),
                license: None,
                lines: Vec::new(),
            });
        } else if line.starts_with("license=") {
            recipe.license = license_list(&line);
            if recipe.license.is_none() {
                problems.push(format!("{path}:{number}: cannot read the license=() list"));
            }
        }
    }
    if let Some(function) = current {
        problems.push(format!(
            "{path}: package_{}() has no closing brace on a line of its own",
            function.name
        ));
    }
    recipe
}

/// Every `packaging/arch/*/PKGBUILD`, sorted.
fn read_recipes(root: &Path, problems: &mut Vec<String>) -> Vec<Recipe> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root.join(RECIPES))
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path().join("PKGBUILD")))
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    if paths.is_empty() {
        problems.push(format!("{RECIPES} holds no */PKGBUILD"));
    }
    paths
        .iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            match std::fs::read_to_string(path) {
                Ok(text) => Some(read_recipe(&rel, &text, problems)),
                Err(error) => {
                    problems.push(format!("{rel}: {error}"));
                    None
                }
            }
        })
        .collect()
}

/// Decodes the character references of XML text.
fn unescape(text: &str) -> String {
    ENTITY
        .replace_all(text, |found: &regex::Captures<'_>| {
            let name = &found[1];
            let decoded = match name {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .map(|hex| u32::from_str_radix(hex, 16).ok())
                    .or_else(|| name.strip_prefix('#').map(|decimal| decimal.parse().ok()))
                    .flatten()
                    .and_then(char::from_u32),
            };
            decoded.map_or_else(|| found[0].to_owned(), String::from)
        })
        .into_owned()
}

/// Removes the indentation every non-blank line shares; blank lines become empty.
fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| {
            if line.chars().all(|c| c == ' ' || c == '\t') {
                ""
            } else {
                line
            }
        })
        .collect();
    let mut margin: Option<&str> = None;
    for line in lines.iter().filter(|line| !line.is_empty()) {
        let indent = &line[..line.len() - line.trim_start_matches([' ', '\t']).len()];
        margin = Some(match margin {
            None => indent,
            Some(margin) => {
                let shared = margin
                    .bytes()
                    .zip(indent.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                &margin[..shared]
            }
        });
    }
    let margin = margin.unwrap_or("");
    lines
        .iter()
        .map(|line| line.strip_prefix(margin).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Paths, relative to a crate's directory, of the files whose notice must ship.
fn embedded_notice_files(package: &Package) -> Vec<String> {
    if SCANNER_CRATES.contains(&package.name.as_str()) {
        let mut sources = Vec::new();
        crate::gate::rust_files(&package.directory().join("src"), &mut sources);
        let mut names = BTreeSet::new();
        for source in sources {
            let Ok(text) = std::fs::read_to_string(&source) else {
                continue;
            };
            for found in XML_LITERAL.captures_iter(&text) {
                let mut name = &found[1];
                while let Some(rest) = name.strip_prefix("./") {
                    name = rest;
                }
                names.insert(name.to_owned());
            }
        }
        return names.into_iter().collect();
    }
    EMBEDDED_NOTICES
        .iter()
        .find(|(name, _)| *name == package.name)
        .map(|(_, files)| files.iter().map(|file| (*file).to_owned()).collect())
        .unwrap_or_default()
}

/// `[(file, notice)]` of each file from `embedded_notice_files`: the leading comment of a registry file, else its
/// `<copyright>`. A protocol file may carry no notice; a registry file must.
fn embedded_notices(package: &Package) -> Result<Vec<(String, String)>, String> {
    let scanner = SCANNER_CRATES.contains(&package.name.as_str());
    let mut found = Vec::new();
    for name in embedded_notice_files(package) {
        let Ok(xml) = std::fs::read_to_string(package.directory().join(&name)) else {
            return Err(format!("{}: {name} is missing", package.label()));
        };
        let captured = REGISTRY_COMMENT
            .captures(&xml)
            .or_else(|| COPYRIGHT.captures(&xml));
        let raw = captured
            .as_ref()
            .and_then(|captured| {
                captured
                    .iter()
                    .skip(1)
                    .flatten()
                    .map(|group| group.as_str())
                    .find(|group| !group.is_empty())
            })
            .unwrap_or("");
        let text = dedent(&unescape(raw)).trim().to_owned();
        if text.contains("opyright") {
            found.push((name, text));
        } else if !scanner {
            return Err(format!(
                "{}: {name} has no copyright notice",
                package.label()
            ));
        }
    }
    Ok(found)
}

/// `[(file name, text)]` of the NOTICE files at the top of a crate's directory.
fn notice_files(package: &Package) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(package.directory()) else {
        return Vec::new();
    };
    let mut found: Vec<(String, String)> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            if !NOTICE_FILE.is_match(&name) {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            Some((name, text.trim().to_owned()))
        })
        .collect();
    found.sort();
    found
}

/// The notices of the registry crates, by package id.
struct Notices {
    files: HashMap<String, Vec<(String, String)>>,
    embedded: HashMap<String, Vec<(String, String)>>,
}

impl Notices {
    fn gather(packages: &[&Package], problems: &mut Vec<String>) -> Self {
        let mut notices = Self {
            files: HashMap::new(),
            embedded: HashMap::new(),
        };
        for package in packages.iter().filter(|package| !package.workspace()) {
            notices
                .files
                .insert(package.id.clone(), notice_files(package));
            match embedded_notices(package) {
                Ok(found) => {
                    notices.embedded.insert(package.id.clone(), found);
                }
                Err(error) => problems.push(error),
            }
        }
        notices
    }

    /// The SPDX identifiers the embedded notices of `packages` name in `SPDX-License-Identifier:` lines.
    fn licences(&self, packages: &[&Package]) -> BTreeSet<String> {
        packages
            .iter()
            .filter_map(|package| self.embedded.get(&package.id))
            .flatten()
            .flat_map(|(_, body)| SPDX_LINE.captures_iter(body))
            .flat_map(|found| identifiers(&found[1]))
            .collect()
    }
}

/// The SPDX identifiers a package ships: those of the workspace crates in the closure, which are linked statically,
/// plus the licences the embedded notices there name.
fn shipped_licences(packages: &[&Package], notices: &Notices) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = packages
        .iter()
        .filter(|package| package.workspace())
        .flat_map(|package| identifiers(package.license.as_deref().unwrap_or("")))
        .collect();
    found.extend(notices.licences(packages));
    found
}

/// Licences of the SDK's closure without their text in the package: an SPDX identifier of a workspace crate needs the
/// text of that name (`TEXT_NAMES`), and a path crate with only a licence file needs that file.
fn sdk_text_problems(
    root: &Path,
    place: &str,
    packages: &[&Package],
    installs: &BTreeMap<String, String>,
    problems: &mut Vec<String>,
) {
    let sources: BTreeSet<PathBuf> = installs
        .values()
        .filter_map(|source| root.join(source).canonicalize().ok())
        .collect();
    for package in packages.iter().filter(|package| package.workspace()) {
        for identifier in identifiers(package.license.as_deref().unwrap_or("")) {
            let name = TEXT_NAMES
                .iter()
                .find(|(spdx, _)| *spdx == identifier)
                .map_or(identifier.as_str(), |(_, name)| name);
            if !installs.contains_key(name) {
                problems.push(format!(
                    "the SDK ships {} ({identifier}) without the text of {identifier} ({place})",
                    package.name
                ));
            }
        }
        if package.license.is_none() {
            if let Some(file) = &package.license_file {
                let shipped = package
                    .directory()
                    .join(file)
                    .canonicalize()
                    .is_ok_and(|path| sources.contains(&path));
                if !shipped {
                    problems.push(format!(
                        "the SDK ships {} without its licence file {file} ({place})",
                        package.name
                    ));
                }
            }
        }
    }
}

/// The workspace crates a package function's closure starts at: the binaries it installs (runtime) or the library
/// (SDK).
fn function_roots(
    graph: &Graph,
    place: &str,
    kind: Kind,
    function: &Function,
    problems: &mut Vec<String>,
) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    let (pattern, target, what) = match kind {
        Kind::Runtime => (
            &*BINARIES,
            "bin",
            "`for name in ...; do` loop over the binaries",
        ),
        Kind::Sdk => (&*LIBRARY, "cdylib", "install line of release/lib<name>.so"),
        Kind::Data => return roots,
    };
    let Some(found) = function
        .lines
        .iter()
        .find_map(|line| pattern.captures(line))
    else {
        problems.push(format!("{place} has no {what}"));
        return roots;
    };
    for name in found[1].split_whitespace() {
        match graph.member_with(target, name) {
            Some(package) => {
                roots.insert(package.label());
            }
            None => problems.push(format!(
                "{place} installs {name}, which no workspace crate builds as a {target} target"
            )),
        }
    }
    roots
}

/// The licence texts a package function installs, `{name under $lic: source}`.
fn licence_installs(
    place: &str,
    function: &Function,
    problems: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut installs = BTreeMap::new();
    for line in &function.lines {
        if LICENCE_DIRECTORY.is_match(line) {
            continue;
        }
        if line.contains("$lic") || line.contains("/usr/share/licenses/") {
            match LICENCE_INSTALL.captures(line) {
                Some(found) => {
                    installs.insert(found[2].to_owned(), found[1].to_owned());
                }
                None => problems.push(format!(
                    "{place}: cannot read the licence install line `{line}` \
                     (expected `install -Dm644 <path> \"$lic/<name>\"`)"
                )),
            }
        }
    }
    if installs.is_empty() {
        problems.push(format!("{place} installs no licence text"));
    }
    installs
}

fn recipe_problems(
    root: &Path,
    graph: &Graph,
    notices: &Notices,
    recipe: &Recipe,
    problems: &mut Vec<String>,
) {
    for kind in [Kind::Runtime, Kind::Sdk] {
        if !recipe
            .functions
            .iter()
            .any(|function| Kind::of(&function.name) == Some(kind))
        {
            problems.push(format!(
                "{} has no package function of the {}",
                recipe.path,
                kind.label()
            ));
        }
    }
    for function in &recipe.functions {
        let place = format!("{} package_{}()", recipe.path, function.name);
        let Some(kind) = Kind::of(&function.name) else {
            problems.push(format!(
                "{place} is not the runtime, the data pack or the SDK"
            ));
            continue;
        };
        let Some(directory) = function
            .lines
            .iter()
            .find_map(|line| LICENCE_DIRECTORY.captures(line))
            .map(|found| found[1].to_owned())
        else {
            problems.push(format!(
                "{place} sets no lic=\"$pkgdir/usr/share/licenses/<package>\""
            ));
            continue;
        };
        let installs = licence_installs(&place, function, problems);
        for (name, source) in &installs {
            if !root.join(source).is_file() {
                problems.push(format!(
                    "licence text {source} for /usr/share/licenses/{directory}/{name} does not exist ({})",
                    recipe.path
                ));
            }
        }
        if kind != Kind::Data {
            for required in [THIRD_PARTY, NOTICES] {
                if !installs.values().any(|source| source == required) {
                    problems.push(format!("{place} does not install {required}"));
                }
            }
        }
        let Some(declared) = function.license.as_ref().or(recipe.license.as_ref()) else {
            problems.push(format!("{place} has no license=() list"));
            continue;
        };
        let roots = function_roots(graph, &place, kind, function, problems);
        let packages = graph.closure(&roots);
        let shipped = match kind {
            Kind::Data => DATA_LICENSES.iter().map(|id| (*id).to_owned()).collect(),
            Kind::Runtime | Kind::Sdk => shipped_licences(&packages, notices),
        };
        let missing: Vec<&str> = shipped.difference(declared).map(String::as_str).collect();
        if !missing.is_empty() {
            problems.push(format!(
                "{} licence: {} {} lacks {}",
                kind.label(),
                recipe.path,
                function.name,
                missing.join(", ")
            ));
        }
        if kind == Kind::Sdk {
            sdk_text_problems(root, &recipe.path, &packages, &installs, problems);
        }
    }
}

/// The `name version` lines of the CRATES list of `THIRD_PARTY`, and the crates its licence texts are used by.
fn read_third_party(text: &str) -> Option<(BTreeSet<String>, BTreeSet<String>)> {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|line| {
        *line == "CRATES"
            || line
                .strip_prefix("CRATES (")
                .is_some_and(|rest| rest.ends_with(')'))
    })?;
    let bar = lines.get(start + 1)?;
    if bar.is_empty() || !bar.chars().all(|c| c == '=') {
        return None;
    }
    let crates = lines[start + 2..]
        .iter()
        .take_while(|line| !line.is_empty())
        .map(|line| (*line).to_owned())
        .collect();
    let used = lines
        .iter()
        .filter_map(|line| line.strip_prefix("Used by: "))
        .flat_map(|users| users.split(", "))
        .map(|user| user.trim().to_owned())
        .collect();
    Some((crates, used))
}

/// What makes `THIRD_PARTY` or `NOTICES` stale: a crate the workspace depends on missing from the list or one it no
/// longer depends on, a listed crate without a licence text, a NOTICE file or an embedded notice not in `NOTICES`.
fn third_party_problems(
    root: &Path,
    packages: &[&Package],
    notices: &Notices,
    problems: &mut Vec<String>,
) {
    let Ok(text) = std::fs::read_to_string(root.join(THIRD_PARTY)) else {
        problems.push(format!("{THIRD_PARTY} is missing: run `{REGENERATE}`"));
        return;
    };
    let Some((listed, used)) = read_third_party(&text) else {
        problems.push(format!(
            "{THIRD_PARTY} has no CRATES list: run `{REGENERATE}`"
        ));
        return;
    };
    let wanted: BTreeSet<String> = packages
        .iter()
        .filter(|package| !package.workspace())
        .map(|package| package.label())
        .collect();
    let before = problems.len();
    for name in wanted.difference(&listed) {
        problems.push(format!("{THIRD_PARTY} lacks {name}"));
    }
    for name in listed.difference(&wanted) {
        problems.push(format!(
            "{THIRD_PARTY} lists {name}, which the workspace does not depend on"
        ));
    }
    for name in listed.difference(&used) {
        problems.push(format!("{THIRD_PARTY} has no licence text used by {name}"));
    }
    if problems.len() > before {
        problems.push(format!("{THIRD_PARTY} is stale: run `{REGENERATE}`"));
    }
    let Ok(notice_text) = std::fs::read_to_string(root.join(NOTICES)) else {
        problems.push(format!("{NOTICES} is missing"));
        return;
    };
    for package in packages.iter().filter(|package| !package.workspace()) {
        for (name, body) in notices.files.get(&package.id).into_iter().flatten() {
            if !notice_text.contains(body.as_str()) {
                problems.push(format!("{NOTICES} lacks the {name} of {}", package.label()));
            }
        }
        for (name, body) in notices.embedded.get(&package.id).into_iter().flatten() {
            if !notice_text.contains(body.as_str()) {
                problems.push(format!(
                    "{NOTICES} lacks the licence comment of {name} in {}",
                    package.label()
                ));
            }
        }
    }
    let headings: BTreeSet<&str> = NOTICE_HEADING
        .captures_iter(&notice_text)
        .chain(NOTICE_HEADING.captures_iter(&text))
        .filter_map(|found| found.get(1).map(|id| id.as_str()))
        .collect();
    for identifier in notices.licences(packages) {
        if !headings.contains(identifier.as_str()) {
            problems.push(format!(
                "{NOTICES} has no text of {identifier}, which an embedded notice names"
            ));
        }
    }
}

/// The licence problems of the tree at `root`.
pub fn problems(root: &Path) -> Vec<String> {
    let graph = match Graph::read(root) {
        Ok(graph) => graph,
        Err(error) => return vec![format!("licences: {error}")],
    };
    let mut problems = Vec::new();
    let workspace = graph.closure(&graph.roots);
    let notices = Notices::gather(&workspace, &mut problems);
    for recipe in read_recipes(root, &mut problems) {
        recipe_problems(root, &graph, &notices, &recipe, &mut problems);
    }
    third_party_problems(root, &workspace, &notices, &mut problems);
    problems
        .into_iter()
        .map(|problem| format!("licences: {problem}"))
        .collect()
}
