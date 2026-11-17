//! `cupertino-layers` at its public boundary: small workspaces checked through the library and the binary.

mod common;

use common::{run_in, stderr, stdout, Fixture};
use cupertino_layers::Layer::{
    Apps, Core, Foundation, Graphics, Models, Protocol, Services, Shell, Testing, Ui,
};
use cupertino_layers::{Policy, POLICY};

fn kinds(fx: &Fixture) -> Vec<&'static str> {
    fx.check().problems.iter().map(|p| p.kind()).collect()
}

// the layer table

#[test]
fn a_dependency_the_row_forbids_is_a_problem() {
    let fx = Fixture::new("forbidden")
        .krate("f", Foundation, &[("normal", "g"), ("build", "g")])
        .krate("g", Graphics, &[])
        .build();
    assert_eq!(
        fx.lines(),
        [
            "foundation forbidden_edge f -> g build",
            "foundation forbidden_edge f -> g normal"
        ]
    );
}

#[test]
fn a_dependency_the_row_allows_is_not_a_problem() {
    let fx = Fixture::new("allowed")
        .krate("f", Foundation, &[])
        .krate("f2", Foundation, &[("normal", "f")])
        .krate("g", Graphics, &[("normal", "f"), ("dev", "f2")])
        .build();
    assert_eq!(fx.lines(), Vec::<String>::new());
    assert!(fx.check().is_clean());
}

#[test]
fn a_dev_dependency_on_a_testing_crate_is_allowed_and_a_normal_one_is_not() {
    let fx = Fixture::new("testing")
        .krate("t", Testing, &[("normal", "f")])
        .krate("f", Foundation, &[("dev", "t")])
        .krate("g", Graphics, &[("normal", "t")])
        .build();
    assert_eq!(fx.lines(), ["graphics forbidden_edge g -> t normal"]);
}

#[test]
fn a_crate_reaches_up_through_a_chain() {
    // ui -> testing is a dev-dependency every crate may have, and the testing crate depends on core
    let fx = Fixture::new("reaches-up")
        .krate("u", Ui, &[("dev", "t")])
        .krate("t", Testing, &[("normal", "c")])
        .krate("c", Core, &[])
        .build();
    assert_eq!(fx.lines(), ["ui reaches_up u -> c"]);
}

#[test]
fn a_chain_that_stays_at_or_below_the_layer_is_not_reaching_up() {
    let fx = Fixture::new("reaches-level")
        .krate("u", Ui, &[("dev", "t")])
        .krate("t", Testing, &[("normal", "u2")])
        .krate("u2", Ui, &[])
        .build();
    assert!(fx.check().is_clean());
}

// purity

fn impure_lines(layer: cupertino_layers::Layer, src: &str) -> Vec<String> {
    let fx = Fixture::new(&format!("pure-{}", src.len()))
        .krate("p", layer, &[])
        .file("p", "src/lib.rs", src)
        .build();
    fx.lines()
}

#[test]
fn std_fs_and_std_env_are_impure_in_foundation() {
    assert_eq!(
        impure_lines(Foundation, "fn f() { let _ = std::fs::read(\"x\"); }"),
        ["foundation impure_source p src/lib.rs std::fs"]
    );
    assert_eq!(
        impure_lines(Models, "use std::env;\nfn f() { let _ = env::args(); }"),
        ["models impure_source p src/lib.rs std::env"]
    );
}

#[test]
fn a_use_group_naming_env_is_impure() {
    assert_eq!(
        impure_lines(Foundation, "use std::{io, env};"),
        ["foundation impure_source p src/lib.rs std::env"]
    );
    assert_eq!(
        impure_lines(Foundation, "use std::{io, fmt};"),
        Vec::<String>::new()
    );
}

#[test]
fn unsafe_and_ffi_are_impure() {
    assert_eq!(
        impure_lines(Foundation, "fn f() { unsafe { } }"),
        ["foundation impure_source p src/lib.rs unsafe"]
    );
    assert_eq!(
        impure_lines(Models, "extern \"C\" { fn abs(x: i32) -> i32; }"),
        ["models impure_source p src/lib.rs ffi"]
    );
    assert_eq!(
        impure_lines(Models, "#[no_mangle]\npub fn f() {}"),
        ["models impure_source p src/lib.rs ffi"]
    );
}

#[test]
fn one_problem_per_file_and_item() {
    let lines = impure_lines(
        Foundation,
        "fn a() { std::fs::read(\"a\"); std::fs::read(\"b\"); }\nfn b() { unsafe {} unsafe {} }",
    );
    assert_eq!(
        lines,
        [
            "foundation impure_source p src/lib.rs std::fs",
            "foundation impure_source p src/lib.rs unsafe"
        ]
    );
}

#[test]
fn comments_and_strings_are_not_impure() {
    let src = "// std::fs::read and unsafe\n/* std::env */\nconst A: &str = \"std::fs unsafe\";\n\
               const B: &str = r#\"std::process\"#;\n/// unsafe docs\nfn f() {}";
    assert_eq!(impure_lines(Foundation, src), Vec::<String>::new());
}

#[test]
fn build_scripts_and_tests_are_exempt_from_purity() {
    let fx = Fixture::new("pure-exempt")
        .krate("p", Foundation, &[])
        .file(
            "p",
            "build.rs",
            "fn main() { let _ = std::env::var(\"X\"); }",
        )
        .file("p", "build/gen.rs", "fn g() { unsafe {} }")
        .file(
            "p",
            "tests/t.rs",
            "fn t() { let _ = std::fs::read(\"x\"); }",
        )
        .build();
    assert_eq!(fx.lines(), Vec::<String>::new());
}

#[test]
fn a_source_in_a_module_directory_is_checked() {
    let fx = Fixture::new("pure-nested")
        .krate("p", Foundation, &[])
        .file("p", "src/a/b.rs", "use std::net::TcpStream;")
        .build();
    assert_eq!(
        fx.lines(),
        ["foundation impure_source p src/a/b.rs std::net"]
    );
}

#[test]
fn other_layers_may_be_impure() {
    assert_eq!(
        impure_lines(
            Graphics,
            "fn f() { let _ = std::fs::read(\"x\"); unsafe {} }"
        ),
        Vec::<String>::new()
    );
}

// facades, components, window system, external crates, licences

#[test]
fn a_shell_or_apps_crate_reaches_the_toolkit_through_its_facade() {
    let fx = Fixture::new("facade")
        .krate(
            "s",
            Shell,
            &[("normal", "cupertino-ui-core"), ("normal", "cupertino-kit")],
        )
        .krate("a", Apps, &[("dev", "cupertino-protocols")])
        .krate("cupertino-ui-core", Ui, &[])
        .krate("cupertino-kit", Ui, &[])
        .krate("cupertino-protocols", Protocol, &[])
        .build();
    assert_eq!(
        fx.lines(),
        [
            "apps facade a -> cupertino-protocols dev",
            "shell facade s -> cupertino-ui-core normal"
        ]
    );
}

#[test]
fn a_services_crate_may_use_the_crates_behind_the_facades() {
    let fx = Fixture::new("facade-services")
        .krate("s", Services, &[("normal", "cupertino-protocols")])
        .krate("cupertino-protocols", Protocol, &[])
        .build();
    assert!(fx.check().is_clean());
}

#[test]
fn shell_crates_stay_apart_except_the_host_on_its_components() {
    let fx = Fixture::new("components")
        .krate("cupertino-dock", Shell, &[("normal", "cupertino-menubar")])
        .krate("cupertino-menubar", Shell, &[])
        .krate(
            "cupertino-shell",
            Shell,
            &[("normal", "cupertino-dock"), ("normal", "cupertino-login")],
        )
        .krate("cupertino-login", Shell, &[])
        .build();
    assert_eq!(
        fx.lines(),
        [
            "shell components_apart cupertino-dock -> cupertino-menubar normal",
            "shell components_apart cupertino-shell -> cupertino-login normal"
        ]
    );
}

#[test]
fn the_window_system_boundary_holds_through_a_chain() {
    let fx = Fixture::new("window-system")
        .krate("cupertino-kit", Ui, &[("normal", "mid")])
        .krate("mid", Ui, &[("normal", "cupertino-protocols")])
        .krate("cupertino-protocols", Protocol, &[])
        .krate("cupertino-other", Ui, &[("normal", "mid")])
        .build();
    assert_eq!(
        fx.lines(),
        ["ui window_system cupertino-kit -> cupertino-protocols"]
    );
}

#[test]
fn a_dev_dependency_does_not_cross_the_window_system_boundary() {
    let fx = Fixture::new("window-system-dev")
        .krate("cupertino-kit", Ui, &[("dev", "cupertino-protocols")])
        .krate("cupertino-protocols", Protocol, &[])
        .build();
    assert!(kinds(&fx).iter().all(|k| *k != "window_system"));
}

#[test]
fn an_external_crate_outside_its_allowed_layers_is_a_problem() {
    let fx = Fixture::new("external")
        .krate("u", Ui, &[("normal", "smithay")])
        .krate("c", Core, &[("normal", "smithay")])
        .krate("f", Foundation, &[("normal", "zbus")])
        .krate("m", Models, &[("normal", "tiny-skia")])
        .krate(
            "m2",
            Models,
            &[("dev", "tiny-skia"), ("dev", "wayland-server")],
        )
        .build();
    assert_eq!(
        fx.lines(),
        [
            "foundation external_crate f -> zbus normal",
            "models external_crate m -> tiny-skia normal",
            "ui external_crate u -> smithay normal"
        ]
    );
}

#[test]
fn a_missing_or_malformed_licence_is_a_problem() {
    let fx = Fixture::new("licence")
        .krate("none", Foundation, &[])
        .krate("bad", Foundation, &[])
        .krate("fine", Foundation, &[])
        .krate("file", Foundation, &[])
        .licence("none", None)
        .licence("bad", Some("all rights reserved"))
        .licence("fine", Some("MIT OR Apache-2.0"))
        .licence("file", Some("file:LICENSE"))
        .build();
    assert_eq!(
        fx.lines(),
        [
            "foundation undeclared_licence bad",
            "foundation undeclared_licence none"
        ]
    );
}

// the remaining-work list

fn one_problem(name: &str) -> Fixture {
    Fixture::new(name)
        .krate("f", Foundation, &[("normal", "g")])
        .krate("g", Graphics, &[])
}

const LISTED: &str = "foundation forbidden_edge f -> g normal";

#[test]
fn a_problem_the_list_does_not_name_fails() {
    let fx = one_problem("unlisted").build();
    let out = fx.run(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("f (foundation) must not depend on g (graphics)"));
    assert!(stdout(&out).ends_with("1 problems\n"), "{}", stdout(&out));
    assert_eq!(fx.check().unlisted().count(), 1);
}

#[test]
fn a_listed_problem_passes() {
    let fx = one_problem("listed").remaining(&[LISTED]).build();
    let out = fx.run(&[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).ends_with("1 listed, 0 problems\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn a_line_that_is_no_longer_a_problem_fails() {
    let fx = one_problem("stale")
        .remaining(&[LISTED, "foundation forbidden_edge f -> h normal"])
        .build();
    let out = fx.run(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("foundation forbidden_edge f -> h normal"));
    let findings = fx.check();
    assert_eq!(findings.unlisted().count(), 0);
    assert_eq!(
        findings
            .listing
            .iter()
            .map(|p| p.kind())
            .collect::<Vec<_>>(),
        ["stale_remaining"]
    );
}

#[test]
fn write_remaining_writes_sorted_lines_and_is_idempotent() {
    let fx = Fixture::new("write-remaining")
        .krate("z", Foundation, &[("normal", "g")])
        .krate("a", Models, &[("normal", "g")])
        .krate("g", Graphics, &[])
        .krate("m", Foundation, &[])
        .file("m", "src/lib.rs", "use std::{io, env};")
        .remaining(&["foundation forbidden_edge old -> gone normal"])
        .build();
    let file = fx.path(cupertino_layers::REMAINING);
    let out = fx.run(&["--write-remaining"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "crates/cupertino-layers/remaining.txt: 3 lines\n"
    );
    let first = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = first.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(first.lines().take_while(|l| l.starts_with('#')).count(), 2);
    assert_eq!(
        lines,
        [
            "foundation forbidden_edge z -> g normal",
            "foundation impure_source m src/lib.rs std::env",
            "models forbidden_edge a -> g normal"
        ]
    );
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    assert!(fx.run(&[]).status.success());
    assert!(fx.run(&["--write-remaining"]).status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), first);
}

#[test]
fn an_enforced_layer_is_never_excused_by_the_list() {
    let fx = one_problem("enforced").remaining(&[LISTED]).build();
    let policy = Policy {
        enforced: &[Foundation],
        ..POLICY
    };
    let findings = fx.check_with(&policy);
    assert_eq!(findings.unlisted().count(), 1);
    assert_eq!(
        findings
            .listing
            .iter()
            .map(|p| p.kind())
            .collect::<Vec<_>>(),
        ["enforced_remaining"]
    );
    assert!(!findings.is_clean());
    // the file written for it leaves the layer's problem out, and writing fails while it remains
    assert!(!findings.remaining_file().contains("forbidden_edge"));
    assert_eq!(findings.problems_writing_the_remaining_work().count(), 1);
    // a layer that is not enforced is excused as before
    assert!(fx.check().is_clean());
}

// the clippy.toml of shell and apps crates

fn two_apps(name: &str) -> Fixture {
    Fixture::new(name)
        .krate("one", Apps, &[])
        .krate("two", Apps, &[])
        .krate("lib", Foundation, &[])
        .no_clippy("one")
        .no_clippy("two")
        .remaining(&[])
        .build()
}

#[test]
fn a_missing_clippy_toml_is_a_listed_problem() {
    let fx = two_apps("clippy-missing");
    assert_eq!(
        fx.lines(),
        [
            "apps disallowed_methods one clippy.toml",
            "apps disallowed_methods two clippy.toml"
        ]
    );
    assert!(!fx.run(&[]).status.success());
    assert!(fx.run(&["--write-remaining"]).status.success());
    assert!(fx.run(&[]).status.success());
}

#[test]
fn a_clippy_toml_that_differs_is_a_problem() {
    let fx = two_apps("clippy-differs");
    std::fs::write(
        fx.path("crates/one/clippy.toml"),
        "disallowed-methods = []\n",
    )
    .unwrap();
    assert!(fx
        .lines()
        .contains(&"apps disallowed_methods one clippy.toml".to_owned()));
}

#[test]
fn write_clippy_with_a_crate_writes_only_that_crate() {
    let fx = two_apps("clippy-one");
    let out = fx.run(&["--write-clippy", "one"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).trim_end().ends_with("crates/one/clippy.toml"));
    assert!(fx.path("crates/one/clippy.toml").is_file());
    assert!(!fx.path("crates/two/clippy.toml").exists());
    assert_eq!(fx.lines(), ["apps disallowed_methods two clippy.toml"]);
}

#[test]
fn write_clippy_without_names_writes_every_shell_and_apps_crate() {
    let fx = two_apps("clippy-all");
    assert!(fx.run(&["--write-clippy"]).status.success());
    assert!(fx.path("crates/one/clippy.toml").is_file());
    assert!(fx.path("crates/two/clippy.toml").is_file());
    assert!(!fx.path("crates/lib/clippy.toml").exists());
    assert!(fx.check().is_clean());
}

#[test]
fn write_clippy_with_an_unknown_name_fails_and_writes_nothing() {
    let fx = two_apps("clippy-unknown");
    for names in [&["nope"][..], &["one", "nope"], &["lib"]] {
        let mut args = vec!["--write-clippy"];
        args.extend(names);
        let out = fx.run(&args);
        assert!(!out.status.success(), "{names:?}");
        assert!(stderr(&out).contains("is not a shell or apps crate"));
    }
    assert!(!fx.path("crates/one/clippy.toml").exists());
    assert!(!fx.path("crates/two/clippy.toml").exists());
    assert!(!fx.path("crates/lib/clippy.toml").exists());
}

// the repository

#[test]
fn the_real_workspace_passes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = run_in(&root, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).trim_end().ends_with(", 0 problems"),
        "{}",
        stdout(&out)
    );
}
