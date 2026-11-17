//! `cupertino-layers` at its public boundary: small workspaces checked through the library and the binary.

mod common;

use common::{run_in, stderr, stdout, Fixture};
use cupertino_layers::Layer::{
    Apps, Core, Foundation, Graphics, Models, Protocol, Services, Shell, System, Testing, Ui,
};
use cupertino_layers::{Layer, Policy, POLICY};

const NONE: &[&str] = &[];

fn assert_lines(fx: &Fixture, expected: &[&str], row: &str) {
    assert_eq!(fx.lines(), expected, "{row}");
}

// the layer table

#[test]
fn a_dependency_counts_by_the_row_of_its_layer() {
    let rows: [(&str, Fixture, &[&str]); 3] = [
        (
            "a row forbids the layer above",
            Fixture::new("forbidden")
                .krate("f", Foundation, &[("normal", "g"), ("build", "g")])
                .krate("g", Graphics, &[]),
            &[
                "foundation forbidden_edge f -> g build",
                "foundation forbidden_edge f -> g normal",
            ],
        ),
        (
            "a row allows its own layer and those below",
            Fixture::new("allowed")
                .krate("f", Foundation, &[])
                .krate("f2", Foundation, &[("normal", "f")])
                .krate("g", Graphics, &[("normal", "f"), ("dev", "f2")]),
            NONE,
        ),
        (
            "a testing crate is a dev-dependency only",
            Fixture::new("testing")
                .krate("t", Testing, &[("normal", "f")])
                .krate("f", Foundation, &[("dev", "t")])
                .krate("g", Graphics, &[("normal", "t")]),
            &["graphics forbidden_edge g -> t normal"],
        ),
    ];
    for (row, fx, expected) in rows {
        assert_lines(&fx.build(), expected, row);
    }
}

#[test]
fn a_crate_reaches_up_only_through_a_chain_that_rises() {
    // ui -> testing is a dev-dependency every crate may have; what the testing crate depends on decides
    for (row, top, expected) in [
        ("through core", Core, &["ui reaches_up u -> c"][..]),
        ("at its own layer", Ui, NONE),
    ] {
        let fx = Fixture::new("reaches-up")
            .krate("u", Ui, &[("dev", "t")])
            .krate("t", Testing, &[("normal", "c")])
            .krate("c", top, &[])
            .build();
        assert_lines(&fx, expected, row);
    }
}

// purity

#[test]
fn foundation_and_models_sources_are_pure() {
    let fs = "foundation impure_source p src/lib.rs std::fs";
    let rows: [(Layer, &str, &[&str]); 13] = [
        (Foundation, "fn f() { let _ = std::fs::read(\"x\"); }", &[fs]),
        (
            Models,
            "use std::env;\nfn f() { let _ = env::args(); }",
            &["models impure_source p src/lib.rs std::env"],
        ),
        (
            Foundation,
            "use std::{io, env};",
            &["foundation impure_source p src/lib.rs std::env"],
        ),
        (Foundation, "use std::{io, fmt};", NONE),
        (
            Foundation,
            "fn f() { unsafe { } }",
            &["foundation impure_source p src/lib.rs unsafe"],
        ),
        (
            Models,
            "extern \"C\" { fn abs(x: i32) -> i32; }",
            &["models impure_source p src/lib.rs ffi"],
        ),
        (
            Models,
            "#[no_mangle]\npub fn f() {}",
            &["models impure_source p src/lib.rs ffi"],
        ),
        // one problem per file and item
        (
            Foundation,
            "fn a() { std::fs::read(\"a\"); std::fs::read(\"b\"); }\nfn b() { unsafe {} unsafe {} }",
            &[fs, "foundation impure_source p src/lib.rs unsafe"],
        ),
        // comments and strings
        (
            Foundation,
            "// std::fs::read and unsafe\n/* std::env */\nconst A: &str = \"std::fs unsafe\";\n\
             const B: &str = r#\"std::process\"#;\n/// unsafe docs\nfn f() {}",
            NONE,
        ),
        // a unix socket is std::net, other unix items are not
        (
            Foundation,
            "use std::os::unix::net::UnixStream;",
            &["foundation impure_source p src/lib.rs std::net"],
        ),
        (
            Models,
            "use std::{io, os::unix::{io::RawFd, net::UnixListener as L}};",
            &["models impure_source p src/lib.rs std::net"],
        ),
        (Foundation, "use std::os::unix::io::RawFd;", NONE),
        // other layers may be impure
        (
            Graphics,
            "fn f() { let _ = std::fs::read(\"x\"); unsafe {} }",
            NONE,
        ),
    ];
    for (i, (layer, src, expected)) in rows.into_iter().enumerate() {
        let fx = Fixture::new(&format!("pure-{i}"))
            .krate("p", layer, &[])
            .file("p", "src/lib.rs", src)
            .build();
        assert_lines(&fx, expected, src);
    }
}

#[test]
fn purity_covers_every_source_but_the_crate_root_build_and_tests() {
    let fx = Fixture::new("pure-paths")
        .krate("p", Foundation, &[])
        .file("p", "src/a/b.rs", "use std::net::TcpStream;")
        .file("p", "src/tests/t.rs", "fn t() { std::fs::read(\"x\"); }")
        .file("p", "src/target/x.rs", "use std::env;")
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
        .file("p", "target/debug/build/x.rs", "use std::env;")
        .build();
    assert_lines(
        &fx,
        &[
            "foundation impure_source p src/a/b.rs std::net",
            "foundation impure_source p src/target/x.rs std::env",
            "foundation impure_source p src/tests/t.rs std::fs",
        ],
        "paths",
    );
}

// threads of system crates

#[test]
fn the_src_of_a_system_crate_starts_no_thread() {
    let spawn = "fn f() { std::thread::spawn(|| {}); }";
    let fx = Fixture::new("threads")
        .krate("s", System, &[])
        .krate("v", Services, &[])
        .file(
            "s",
            "src/lib.rs",
            "fn f() { std::thread::spawn(|| {}); }\nfn g() { std::thread::scope(|_| {}); }",
        )
        .file(
            "s",
            "src/a.rs",
            "use std::thread;\nfn f() { thread::Builder::new().spawn(|| {}); thread::spawn(|| {}); }",
        )
        .file("s", "src/b/c.rs", "use std::{io, thread::{self, scope}};")
        // other thread items, comments and strings start no thread
        .file(
            "s",
            "src/quiet.rs",
            "// std::thread::spawn\nconst A: &str = \"thread::spawn\";\n\
             fn f() { std::thread::sleep(std::time::Duration::ZERO); let t = std::thread::current(); \
             rustix::thread::gettid(); }",
        )
        // only the src of a system crate is checked
        .file("s", "tests/t.rs", spawn)
        .file("s", "examples/e.rs", spawn)
        .file("s", "build.rs", spawn)
        .file("s", "benches/b.rs", spawn)
        .file("v", "src/lib.rs", spawn)
        .build();
    assert_lines(
        &fx,
        &[
            "system starts_thread s src/a.rs std::thread::Builder::spawn",
            "system starts_thread s src/a.rs std::thread::spawn",
            "system starts_thread s src/b/c.rs std::thread::scope",
            "system starts_thread s src/lib.rs std::thread::scope",
            "system starts_thread s src/lib.rs std::thread::spawn",
        ],
        "threads",
    );
    let findings = fx.check();
    assert_eq!(findings.unlisted().count(), 5);
}

// facades, components, window system, external crates, licences

#[test]
fn shell_and_apps_crates_reach_the_toolkit_through_its_facade() {
    let fx = Fixture::new("facade")
        .krate(
            "s",
            Shell,
            &[("normal", "cupertino-ui-core"), ("normal", "cupertino-kit")],
        )
        .krate("a", Apps, &[("dev", "cupertino-protocols")])
        .krate("v", Services, &[("normal", "cupertino-protocols")])
        .krate("cupertino-ui-core", Ui, &[])
        .krate("cupertino-kit", Ui, &[])
        .krate("cupertino-protocols", Protocol, &[])
        .build();
    assert_lines(
        &fx,
        &[
            "apps facade a -> cupertino-protocols dev",
            "shell facade s -> cupertino-ui-core normal",
        ],
        "facade",
    );
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
    assert_lines(
        &fx,
        &[
            "shell components_apart cupertino-dock -> cupertino-menubar normal",
            "shell components_apart cupertino-shell -> cupertino-login normal",
        ],
        "components",
    );
}

#[test]
fn the_window_system_boundary_holds_through_a_normal_chain() {
    let rows: [(&str, Fixture, &[&str]); 2] = [
        (
            "through a chain",
            Fixture::new("window-system")
                .krate("cupertino-kit", Ui, &[("normal", "mid")])
                .krate("mid", Ui, &[("normal", "cupertino-protocols")])
                .krate("cupertino-other", Ui, &[("normal", "mid")]),
            &["ui window_system cupertino-kit -> cupertino-protocols"],
        ),
        (
            "a dev-dependency",
            Fixture::new("window-system-dev").krate(
                "cupertino-kit",
                Ui,
                &[("dev", "cupertino-protocols")],
            ),
            &[],
        ),
    ];
    for (row, fx, expected) in rows {
        let fx = fx.krate("cupertino-protocols", Protocol, &[]).build();
        let crossings: Vec<String> = fx
            .lines()
            .into_iter()
            .filter(|l| l.contains(" window_system "))
            .collect();
        assert_eq!(crossings, expected, "{row}");
    }
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
    assert_lines(
        &fx,
        &[
            "foundation external_crate f -> zbus normal",
            "models external_crate m -> tiny-skia normal",
            "ui external_crate u -> smithay normal",
        ],
        "external",
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
    assert_lines(
        &fx,
        &[
            "foundation undeclared_licence bad",
            "foundation undeclared_licence none",
        ],
        "licence",
    );
}

// the remaining-work list

// The fixtures use `shell` and `apps`, the last layers to close, so that closing a layer leaves these tests as they are.
fn one_problem(name: &str) -> Fixture {
    Fixture::new(name)
        .krate("f", Shell, &[("normal", "g")])
        .krate("g", Apps, &[])
}

const LISTED: &str = "shell forbidden_edge f -> g normal";

#[test]
fn the_list_excuses_exactly_the_problems_it_names() {
    // a problem the list does not name fails
    let fx = one_problem("unlisted").build();
    let out = fx.run(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("f (shell) must not depend on g (apps)"));
    assert!(stdout(&out).ends_with("1 problems\n"), "{}", stdout(&out));
    assert_eq!(fx.check().unlisted().count(), 1);
    drop(fx);

    // a listed problem passes
    let fx = one_problem("listed").remaining(&[LISTED]).build();
    let out = fx.run(&[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).ends_with("1 listed, 0 problems\n"),
        "{}",
        stdout(&out)
    );
    drop(fx);

    // a line that is no longer a problem fails
    let stale = "shell forbidden_edge f -> h normal";
    let fx = one_problem("stale").remaining(&[LISTED, stale]).build();
    let out = fx.run(&[]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains(stale));
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
        .krate("z", Shell, &[("normal", "g")])
        .krate("a", Shell, &[("normal", "g")])
        .krate("g", Apps, &[])
        .krate("m", Shell, &[])
        .no_clippy("m")
        .remaining(&["shell forbidden_edge old -> gone normal"])
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
            "shell disallowed_methods m clippy.toml",
            "shell forbidden_edge a -> g normal",
            "shell forbidden_edge z -> g normal"
        ]
    );
    assert!(fx.run(&[]).status.success());
    assert!(fx.run(&["--write-remaining"]).status.success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), first);
}

#[test]
fn an_enforced_layer_is_never_excused_by_the_list() {
    let fx = one_problem("enforced").remaining(&[LISTED]).build();
    let policy = Policy {
        enforced: &[Shell],
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
fn a_missing_or_differing_clippy_toml_is_a_listable_problem() {
    let fx = two_apps("clippy-missing");
    assert_lines(
        &fx,
        &[
            "apps disallowed_methods one clippy.toml",
            "apps disallowed_methods two clippy.toml",
        ],
        "missing",
    );
    assert!(!fx.run(&[]).status.success());
    assert!(fx.run(&["--write-remaining"]).status.success());
    assert!(fx.run(&[]).status.success());
    drop(fx);

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
fn write_clippy_writes_the_named_shell_and_apps_crates() {
    let exists = |fx: &Fixture| {
        ["one", "two", "lib"].map(|k| fx.path(&format!("crates/{k}/clippy.toml")).is_file())
    };

    let fx = two_apps("clippy-one");
    let out = fx.run(&["--write-clippy", "one"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).trim_end().ends_with("crates/one/clippy.toml"));
    assert_eq!(exists(&fx), [true, false, false]);
    assert_lines(&fx, &["apps disallowed_methods two clippy.toml"], "one");
    drop(fx);

    let fx = two_apps("clippy-all");
    assert!(fx.run(&["--write-clippy"]).status.success());
    assert_eq!(exists(&fx), [true, true, false]);
    assert!(fx.check().is_clean());
    drop(fx);

    let fx = two_apps("clippy-unknown");
    for names in [&["nope"][..], &["one", "nope"], &["lib"]] {
        let mut args = vec!["--write-clippy"];
        args.extend(names);
        let out = fx.run(&args);
        assert!(!out.status.success(), "{names:?}");
        assert!(stderr(&out).contains("is not a shell or apps crate"));
    }
    assert_eq!(exists(&fx), [false, false, false]);
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
