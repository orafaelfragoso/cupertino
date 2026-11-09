//! The README's table of event ids is the registry: every id the workspace's sources record or read is in it, with the
//! severities it is recorded at, and it names no id that nothing records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

type Ids = BTreeMap<String, BTreeSet<String>>;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The Rust sources of every crate, tests excluded.
fn sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = std::fs::read_dir(workspace().join("crates"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((path.clone(), std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

/// The text of `source` after each `needle`, with its whitespace collapsed, so a call rustfmt split over lines reads
/// as one.
fn after<'a>(source: &'a str, needle: &'a str) -> impl Iterator<Item = String> + 'a {
    source.match_indices(needle).map(move |(at, _)| {
        let rest = &source[at + needle.len()..];
        let end = rest.len().min(400);
        let mut end = end;
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        rest[..end].split_whitespace().collect::<Vec<_>>().join(" ")
    })
}

/// The first string literal of `text`.
fn literal(text: &str) -> Option<&str> {
    let start = text.find('"')? + 1;
    let len = text[start..].find('"')?;
    Some(&text[start..start + len])
}

/// `Severity::Warn, "id"` at the start of `text`: the severity's name and the id.
fn severity_and_id(text: &str) -> Option<(String, String)> {
    let text = text.trim_start_matches("diag::");
    let rest = text.strip_prefix("Severity::")?;
    let name: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = rest[name.len()..]
        .trim_start()
        .strip_prefix(',')?
        .trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    Some((name.to_lowercase(), literal(rest)?.to_owned()))
}

fn add(ids: &mut Ids, id: &str, severity: &str) {
    ids.entry(id.to_owned())
        .or_default()
        .insert(severity.to_owned());
}

/// Every id the sources of the other crates record or read, with its severities, and the areas of their processes.
fn recorded() -> (Ids, BTreeSet<String>) {
    let mut ids = Ids::new();
    let mut areas = BTreeSet::new();
    // by crate: the areas its sources `init`, and whether any of them records an exit error
    let mut crates: BTreeMap<PathBuf, (BTreeSet<String>, bool)> = BTreeMap::new();
    let root = workspace().join("crates");
    for (path, source) in sources() {
        let krate = root.join(
            path.strip_prefix(&root)
                .unwrap()
                .components()
                .next()
                .unwrap(),
        );
        // its own sources hold examples, not events
        if krate.ends_with("cupertino-diagnostic-events") {
            continue;
        }
        let entry = crates.entry(krate).or_default();
        for call in after(&source, "diag::event(") {
            let (severity, id) = severity_and_id(&call).unwrap_or_else(|| {
                panic!(
                    "{}: an event whose id is not a literal: {call}",
                    path.display()
                )
            });
            add(&mut ids, &id, &severity);
        }
        for call in after(&source, "prepare_fatal(") {
            let id = literal(&call).unwrap_or_else(|| {
                panic!(
                    "{}: a fatal record whose id is not a literal: {call}",
                    path.display()
                )
            });
            add(&mut ids, id, "fatal");
        }
        // `init(component, "area", sink)`
        for call in after(&source, "diag::init(") {
            let area = call
                .split_once(',')
                .and_then(|(_, rest)| literal(rest))
                .unwrap_or_else(|| {
                    panic!(
                        "{}: an init whose area is not a literal: {call}",
                        path.display()
                    )
                });
            entry.0.insert(area.to_owned());
        }
        entry.1 |= source.contains("diag::exit_error(");
        // the session log the reader turns into events: `("session.start", Severity::Info, ...)`
        if path.ends_with("cupertino-diagnostics/src/events.rs") {
            let flat = source
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace("( \"", "(\"");
            for call in after(&flat, "(\"session.") {
                let quote = call.find('"').unwrap();
                let id = format!("session.{}", &call[..quote]);
                let rest = call[quote + 1..].trim_start_matches(',').trim_start();
                if let Some((severity, _)) = severity_and_id(rest) {
                    add(&mut ids, &id, &severity);
                }
            }
        }
    }
    for (krate_areas, exits) in crates.values() {
        for area in krate_areas {
            areas.insert(area.clone());
            add(&mut ids, &format!("{area}.panic"), "fatal");
            if *exits {
                add(&mut ids, &format!("{area}.exit_error"), "error");
            }
        }
    }
    add(
        &mut ids,
        cupertino_diagnostic_events::STORE_UNAVAILABLE,
        "warn",
    );
    (ids, areas)
}

/// The README's table, with `<area>.panic` and `<area>.exit_error` for the areas it names, and those areas.
fn registry() -> (Ids, BTreeSet<String>, BTreeSet<String>) {
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    let begin = readme.find("<!-- event ids: begin -->").unwrap();
    let end = readme.find("<!-- event ids: end -->").unwrap();
    let section = &readme[begin..end];
    let mut ids = Ids::new();
    let mut templates = BTreeMap::new();
    for row in section.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<&str> = row.trim_matches('|').split(" | ").map(str::trim).collect();
        assert_eq!(cells.len(), 5, "{row}");
        let severities: BTreeSet<String> = cells[1].split(", ").map(str::to_owned).collect();
        for id in cells[0].split(", ") {
            let id = id.trim_matches('`');
            if let Some(what) = id.strip_prefix("<area>.") {
                templates.insert(what.to_owned(), severities.clone());
            } else {
                assert!(
                    ids.insert(id.to_owned(), severities.clone()).is_none(),
                    "{id} is in the table twice"
                );
            }
        }
    }
    let areas_text = &section[section.find("The areas:").expect("the areas are listed")..];
    let areas: BTreeSet<String> = areas_text
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|word| !word.starts_with("cupertino-") && !word.starts_with('-'))
        .map(str::to_owned)
        .collect();
    let mut exit_areas = areas.clone();
    // the server records how it ended as `server.session_end`
    exit_areas.remove("server");
    for (what, severities) in &templates {
        let of = if what == "exit_error" {
            &exit_areas
        } else {
            &areas
        };
        for area in of {
            ids.insert(format!("{area}.{what}"), severities.clone());
        }
    }
    (ids, areas, exit_areas)
}

#[test]
fn the_readme_lists_every_event_id_with_its_severities() {
    let (code, code_areas) = recorded();
    let (readme, areas, _) = registry();
    assert_eq!(
        code_areas, areas,
        "the areas of the processes that call init"
    );
    let missing: Vec<_> = code.keys().filter(|id| !readme.contains_key(*id)).collect();
    assert!(
        missing.is_empty(),
        "recorded but not in the README's table: {missing:?}"
    );
    let stale: Vec<_> = readme.keys().filter(|id| !code.contains_key(*id)).collect();
    assert!(
        stale.is_empty(),
        "in the README's table but recorded nowhere: {stale:?}"
    );
    for (id, severities) in &code {
        assert_eq!(&readme[id], severities, "the severities of {id}");
    }
}

#[test]
fn every_id_is_dotted_lower_case_and_starts_with_its_area() {
    let (code, areas) = recorded();
    let known: BTreeSet<&str> = areas
        .iter()
        .map(String::as_str)
        .chain(["diag", "session"])
        .collect();
    for id in code.keys() {
        let parts: Vec<&str> = id.split('.').collect();
        assert!(parts.len() >= 2, "{id}");
        for part in &parts {
            assert!(
                !part.is_empty()
                    && part.starts_with(|c: char| c.is_ascii_lowercase())
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "{id}"
            );
        }
        assert!(known.contains(parts[0]), "{id} starts with no area");
    }
}
