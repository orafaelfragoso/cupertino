//! The manual pages: generated from the registry, the same bytes every time, and clean under `mandoc -T lint`, which
//! a missing mandoc skips unless `CUPERTINO_MANDOC_REQUIRED=1` (CI) makes that a failure.

use std::process::Command as Process;

use cupertino_cli::manual::{self, FIXED_DATE};
use cupertino_cli::{FailureKind, Group, GLOBAL_OPTIONS, REGISTRY};

fn page<'a>(pages: &'a [(String, String)], name: &str) -> &'a str {
    &pages.iter().find(|(n, _)| n == name).unwrap().1
}

/// The text of section `name` of a page: from its `.SH` line to the next.
fn section<'a>(page: &'a str, name: &str) -> &'a str {
    let start = page
        .find(&format!(".SH {name}\n"))
        .unwrap_or_else(|| panic!("no {name}"));
    let rest = &page[start + 1..];
    &rest[..rest.find("\n.SH ").map_or(rest.len(), |i| i + 1)]
}

fn roff(s: &str) -> String {
    s.replace('-', "\\-")
}

#[test]
fn there_is_a_page_for_the_command_and_one_per_group() {
    let pages = manual::pages(REGISTRY, FIXED_DATE);
    let names: Vec<&str> = pages.iter().map(|(n, _)| n.as_str()).collect();
    let mut expected = vec!["cupertino.1".to_owned()];
    expected.extend(REGISTRY.iter().map(|g| format!("cupertino-{}.1", g.name)));
    assert_eq!(names, expected);
    for (name, text) in &pages {
        let title = name.trim_end_matches(".1").to_uppercase();
        assert!(
            text.starts_with(&format!(".TH {} 1 {FIXED_DATE} ", roff(&title))),
            "{name}: {}",
            text.lines().next().unwrap()
        );
        for section in ["NAME", "SYNOPSIS", "DESCRIPTION", "SEE ALSO"] {
            assert!(
                text.contains(&format!("\n.SH {section}\n")),
                "{name}: {section}"
            );
        }
    }
}

#[test]
fn every_command_is_in_its_group_page_synopsis_with_its_arguments() {
    let pages = manual::pages(REGISTRY, FIXED_DATE);
    for group in REGISTRY {
        let text = page(&pages, &format!("cupertino-{}.1", group.name));
        let synopsis = section(text, "SYNOPSIS");
        for command in group.commands() {
            assert!(
                synopsis.contains(&format!("\\fBcupertino {}\\fR", roff(&command.name()))),
                "{}: {synopsis}",
                command.name()
            );
            for arg in command.args {
                // a positional argument with values shows them in place of its name
                let shown = arg.values.first().copied().unwrap_or(arg.name);
                assert!(
                    synopsis.contains(&roff(shown)),
                    "{} {}",
                    command.name(),
                    arg.name
                );
            }
            let commands = section(text, "COMMANDS");
            assert!(commands.contains(&format!(".SS cupertino {}\n", roff(&command.name()))));
        }
    }
    let main = page(&pages, "cupertino.1");
    let exit = section(main, "EXIT STATUS");
    for kind in FailureKind::ALL {
        let entry = format!(
            ".TP\n\\fB{}\\fR (\\fB{}\\fR)\n",
            kind.exit_code(),
            kind.name()
        );
        assert!(exit.contains(&entry), "{entry}");
    }
    for arg in GLOBAL_OPTIONS {
        assert!(section(main, "GLOBAL OPTIONS").contains(&roff(arg.name)));
    }
    let see_also = section(main, "SEE ALSO");
    for group in REGISTRY {
        assert!(see_also.contains(&format!(".BR cupertino\\-{} (1)", roff(group.name))));
    }
}

#[test]
fn two_runs_write_the_same_bytes() {
    assert_eq!(
        manual::pages(REGISTRY, FIXED_DATE),
        manual::pages(REGISTRY, FIXED_DATE)
    );
    assert_eq!(manual::page_date(None), FIXED_DATE);
    assert_eq!(manual::page_date(Some("1700000000".as_ref())), "2023-11-14");
    let empty: &[Group] = &[];
    assert_eq!(manual::pages(empty, FIXED_DATE).len(), 1);
}

#[test]
fn mandoc_finds_nothing_to_warn_about() {
    let found = Process::new("mandoc")
        .arg("-V")
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .is_ok();
    if !found {
        assert_ne!(
            std::env::var("CUPERTINO_MANDOC_REQUIRED").as_deref(),
            Ok("1"),
            "mandoc is required"
        );
        eprintln!("skipped: mandoc is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in manual::pages(REGISTRY, FIXED_DATE) {
        let path = dir.path().join(&name);
        std::fs::write(&path, text).unwrap();
        let out = Process::new("mandoc")
            .args(["-T", "lint", "-W", "warning"])
            .arg(&path)
            .env_clear()
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .output()
            .unwrap();
        assert!(
            out.status.success() && out.stdout.is_empty() && out.stderr.is_empty(),
            "{name}:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
