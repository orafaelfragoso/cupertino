//! The rules of the registry, on the real one and on registries that break each of them.

use cupertino_cli::{
    check_registry, Arg, Args, Command, Context, Failure, Group, Reply, Run, Stability, ValueHint,
    GLOBAL_OPTIONS, REGISTRY, RESERVED,
};

#[test]
fn the_registry_keeps_every_rule() {
    assert_eq!(check_registry(REGISTRY), Vec::<String>::new());
}

#[test]
fn no_reserved_group_is_registered_and_the_reserved_names_are_system_and_ai() {
    let reserved: Vec<&str> = RESERVED.iter().map(|r| r.name).collect();
    assert_eq!(reserved, ["system", "ai"]);
    assert!(REGISTRY.iter().all(|g| !reserved.contains(&g.name)));
}

#[test]
fn every_command_has_a_summary_a_description_an_example_and_a_stability() {
    for c in REGISTRY.iter().flat_map(Group::commands) {
        assert!(
            !c.summary.is_empty() && !c.description.is_empty(),
            "{}",
            c.name()
        );
        assert!(!c.examples.is_empty(), "{}", c.name());
        if c.path[0] == "dev" {
            assert_eq!(c.stability, Stability::Preview, "{}", c.name());
        }
    }
}

#[test]
fn the_global_options_are_registry_data() {
    let names: Vec<&str> = GLOBAL_OPTIONS.iter().map(|o| o.name).collect();
    assert_eq!(names, ["format", "help", "version"]);
    assert_eq!(GLOBAL_OPTIONS[0].values, ["human", "json", "jsonl"]);
}

fn nothing(_: &Args, _: &Context) -> Result<Reply, Failure> {
    Ok(Reply::new(&(), ""))
}

const fn command(path: &'static [&'static str], args: &'static [Arg]) -> Command {
    Command {
        path,
        summary: "s",
        description: "d",
        examples: &[("cupertino x y", "x")],
        stability: Stability::Stable,
        args,
        run: Run::Once(nothing),
    }
}

const fn group(name: &'static str, commands: &'static [&'static [Command]]) -> Group {
    Group {
        name,
        summary: "s",
        description: "d",
        commands,
    }
}

/// One registry per rule, each with exactly the problem it breaks.
#[test]
fn each_broken_rule_is_reported() {
    const OK: &[Command] = &[command(&["x"], &[])];
    const CASES: &[(&[Group], &str)] = &[
        (
            &[
                group("x", &[OK]),
                group("x", &[&[command(&["x", "y"], &[])]]),
            ],
            "group `x` is registered twice",
        ),
        (
            &[Group {
                description: "",
                ..group("x", &[OK])
            }],
            "group `x` needs a name, a summary and a description",
        ),
        (
            &[group(
                "system",
                &[&[Command {
                    examples: &[("cupertino system", "x")],
                    ..command(&["system"], &[])
                }]],
            )],
            "reserved",
        ),
        (
            &[group(
                "help",
                &[&[Command {
                    examples: &[("cupertino help", "x")],
                    ..command(&["help"], &[])
                }]],
            )],
            "a command the runner answers itself",
        ),
        (
            &[group(
                "x",
                &[&[Command {
                    examples: &[("cupertino y", "x")],
                    ..command(&["y"], &[])
                }]],
            )],
            "not a path under its group",
        ),
        (&[group("x", &[OK, OK])], "`x` is registered twice"),
        (
            &[group(
                "x",
                &[&[Command {
                    examples: &[],
                    ..command(&["x"], &[])
                }]],
            )],
            "needs a summary, a description and an example",
        ),
        (
            &[group(
                "x",
                &[&[Command {
                    examples: &[("x --help", "x")],
                    ..command(&["x"], &[])
                }]],
            )],
            "has an example that is not",
        ),
        (
            &[group(
                "dev",
                &[&[Command {
                    examples: &[("cupertino --format json dev y", "x")],
                    ..command(&["dev", "y"], &[])
                }]],
            )],
            "is a dev command, so a preview",
        ),
        (
            &[group(
                "x",
                &[&[command(
                    &["x"],
                    &[Arg::option("a", "h"), Arg::flag("a", "h")],
                )]],
            )],
            "the argument name `a` twice",
        ),
        (
            &[group(
                "x",
                &[&[command(&["x"], &[Arg::option("format", "h")])]],
            )],
            "the argument name `format` twice",
        ),
        (
            &[group(
                "x",
                &[&[command(
                    &["x"],
                    &[Arg::option("a", "h").values(&["b", "b"])],
                )]],
            )],
            "an empty, spaced or repeated value",
        ),
        (
            &[group(
                "x",
                &[&[command(&["x"], &[Arg::option("a", "h").values(&["b c"])])]],
            )],
            "an empty, spaced or repeated value",
        ),
        (
            &[group(
                "x",
                &[&[command(&["x"], &[Arg::flag("a", "h").repeat()])]],
            )],
            "takes no value",
        ),
        (
            &[group(
                "x",
                &[&[command(
                    &["x"],
                    &[Arg::option("a", "h").values(&["b"]).hint(ValueHint::File)],
                )]],
            )],
            "has values, so no hint",
        ),
        (
            &[group(
                "x",
                &[&[command(
                    &["x"],
                    &[
                        Arg::positional("a", "h").repeat(),
                        Arg::positional("b", "h"),
                    ],
                )]],
            )],
            "a repeatable positional argument before another",
        ),
        (
            &[group(
                "x",
                &[&[
                    command(&["x"], &[Arg::positional("a", "h")]),
                    command(&["x", "y"], &[]),
                ]],
            )],
            "begins a longer command path",
        ),
    ];
    for (registry, expected) in CASES {
        let problems = check_registry(registry);
        assert_eq!(problems.len(), 1, "{expected}: {problems:?}");
        assert!(problems[0].contains(expected), "{expected}: {problems:?}");
    }
    const FINE: &[Group] = &[group(
        "x",
        &[&[
            command(&["x"], &[]),
            command(&["x", "y"], &[Arg::positional("a", "h").repeat()]),
        ]],
    )];
    assert_eq!(check_registry(FINE), Vec::<String>::new());
}
