//! The completion scripts: generated from the registry alone, and completing in the real shells. A shell that is not
//! installed skips its part, unless `CUPERTINO_SHELLS_REQUIRED=1` (CI) makes that a failure.

mod support;

use std::path::Path;
use std::process::Command as Process;

use cupertino_cli::completions::{self, SHELLS};
use cupertino_cli::{
    Arg, Args, Command, Context, Failure, Group, Reply, Run, Runner, Stability, BUILTINS,
    GLOBAL_OPTIONS, REGISTRY,
};
use support::{args, envelope};

fn nothing(_: &Args, _: &Context) -> Result<Reply, Failure> {
    Ok(Reply::new(&serde_json::json!({}), ""))
}

/// A registry with a command no real one has, to show the scripts follow the registry.
const PROBE: &[Group] = &[Group {
    name: "probe",
    summary: "A test group",
    description: "A test group.",
    commands: &[&[Command {
        path: &["probe", "widget"],
        summary: "A test command",
        description: "A test command.",
        examples: &[("cupertino probe widget", "Probe.")],
        stability: Stability::Preview,
        args: &[
            Arg::option("colour", "a colour").values(&["red", "green", "blue"]),
            Arg::flag("loud", "loudly"),
            Arg::positional("thing", "a thing").values(&["gear", "spring"]),
        ],
        run: Run::Once(nothing),
    }]],
}];

fn script(shell: &str, registry: &[Group]) -> String {
    completions::script(shell, registry).unwrap()
}

#[test]
fn every_command_path_option_and_value_is_in_every_script() {
    for shell in SHELLS {
        let text = script(shell, REGISTRY);
        for builtin in BUILTINS {
            assert!(
                text.contains(&format!("'{}'", builtin.name)),
                "{shell}: {}",
                builtin.name
            );
        }
        for arg in GLOBAL_OPTIONS {
            assert!(
                text.contains(&format!(" {} ", arg.name)),
                "{shell}: global {}",
                arg.name
            );
        }
        for command in REGISTRY.iter().flat_map(Group::commands) {
            assert!(
                text.contains(&format!("'{}'", command.name())),
                "{shell}: {}",
                command.name()
            );
            for arg in command.args {
                assert!(
                    text.contains(&format!(" {} ", arg.name)),
                    "{shell}: {} {}",
                    command.name(),
                    arg.name
                );
                for value in arg.values {
                    assert!(text.contains(value), "{shell}: {} {value}", arg.name);
                }
            }
        }
        assert!(!text.contains("probe"), "{shell}");
        let probe = script(shell, PROBE);
        for word in [
            "'probe widget'",
            "colour",
            "red,green,blue",
            "loud",
            "gear,spring",
        ] {
            assert!(probe.contains(word), "{shell}: {word}");
        }
        assert!(
            !probe.contains("'desktop start'"),
            "{shell}: the tables are the registry's alone"
        );
    }
    assert_eq!(completions::script("tcsh", REGISTRY), None);
}

#[test]
fn the_command_prints_the_script_of_its_registry() {
    let dir = tempfile::tempdir().unwrap();
    let run = |registry: &'static [Group], words: &[&str]| {
        Runner::new(registry)
            .context(Context::isolated(dir.path()))
            .run(&args(words), "r1")
    };
    for shell in SHELLS {
        let human = run(REGISTRY, &["completions", shell]);
        assert_eq!((human.code, human.stderr.as_str()), (0, ""));
        assert_eq!(human.stdout, script(shell, REGISTRY));
        let json = run(REGISTRY, &["--format", "json", "completions", shell]);
        let v = envelope(&json);
        assert_eq!(v["data"]["shell"], *shell);
        assert_eq!(v["data"]["script"], script(shell, REGISTRY));
    }
    let wrong = run(REGISTRY, &["--format", "json", "completions", "tcsh"]);
    assert_eq!(wrong.code, 2);
    assert_eq!(envelope(&wrong)["error"]["code"], "invalid_value");
}

/// Whether `shell` can run; a missing one fails when the shells are required.
fn have(shell: &str) -> bool {
    let found = Process::new(shell)
        .arg("-c")
        .arg("exit 0")
        .env_clear()
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        assert_ne!(
            std::env::var("CUPERTINO_SHELLS_REQUIRED").as_deref(),
            Ok("1"),
            "{shell} is required"
        );
        eprintln!("skipped: {shell} is not installed");
    }
    found
}

/// Runs `code` in `shell` without its configuration, with a temporary HOME, in `dir`.
fn shell(shell: &str, dir: &Path, code: &str) -> String {
    let mut command = Process::new(shell);
    match shell {
        "bash" => command.args(["--norc", "--noprofile", "-c", code]),
        "zsh" => command.args(["-f", "-c", code]),
        _ => command.args(["--no-config", "-c", code]),
    };
    let out = command
        .current_dir(dir)
        .env_clear()
        .env("HOME", dir)
        .env("PATH", "/usr/bin:/bin")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{shell}: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// What the bash script offers for `line`, the cursor at its end.
fn bash_offers(dir: &Path, registry: &[Group], line: &str) -> Vec<String> {
    std::fs::write(dir.join("cupertino.bash"), script("bash", registry)).unwrap();
    let code = format!(
        "source ./cupertino.bash\n\
         line={}\n\
         read -ra COMP_WORDS <<<\"$line\"\n\
         [[ $line == *' ' ]] && COMP_WORDS+=('')\n\
         COMP_CWORD=$((${{#COMP_WORDS[@]}} - 1))\n\
         _cupertino\n\
         printf '%s\\n' \"${{COMPREPLY[@]}}\"",
        sh(line)
    );
    let mut offers: Vec<String> = shell("bash", dir, &code)
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    offers.sort();
    offers
}

fn sh(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[test]
fn bash_completes_commands_options_and_values() {
    if !have("bash") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(bash_offers(d, REGISTRY, "cupertino dia"), ["diagnose"]);
    assert_eq!(bash_offers(d, REGISTRY, "cupertino de"), ["desktop", "dev"]);
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --format json desk"),
        ["desktop"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino --format "),
        ["human", "json", "jsonl"]
    );
    assert_eq!(bash_offers(d, REGISTRY, "cupertino desktop "), ["start"]);
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --area g"),
        ["gpu"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino desktop start --backend "),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --"),
        ["--area", "--format", "--help", "--strict", "--version"]
    );
    // a single option is offered once, a repeatable one again
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --strict --"),
        ["--area", "--format", "--help", "--version"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino diagnose --area gpu --"),
        ["--area", "--format", "--help", "--strict", "--version"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino completions "),
        ["bash", "fish", "zsh"]
    );
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino help dev "),
        ["layers", "manual"]
    );
    assert_eq!(bash_offers(d, REGISTRY, "cupertino he"), ["help"]);

    std::fs::create_dir(d.join("pages")).unwrap();
    std::fs::write(d.join("pages.txt"), "").unwrap();
    assert_eq!(
        bash_offers(d, REGISTRY, "cupertino dev manual pa"),
        ["pages"],
        "directories alone"
    );

    assert_eq!(bash_offers(d, PROBE, "cupertino pr"), ["probe"]);
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget --colour "),
        ["blue", "green", "red"]
    );
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget --loud "),
        ["gear", "spring"]
    );
    assert_eq!(
        bash_offers(d, PROBE, "cupertino probe widget gear "),
        Vec::<String>::new()
    );
}

#[test]
fn bash_registers_the_function_for_the_command() {
    if !have("bash") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("c.bash"), script("bash", REGISTRY)).unwrap();
    assert_eq!(
        shell("bash", dir.path(), "source ./c.bash; complete -p cupertino"),
        "complete -F _cupertino cupertino\n"
    );
}

#[test]
fn zsh_reads_the_script_and_registers_it() {
    if !have("zsh") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("_cupertino"), script("zsh", REGISTRY)).unwrap();
    shell("zsh", dir.path(), "zsh -n ./_cupertino");
    let registered = shell(
        "zsh",
        dir.path(),
        "autoload -Uz compinit && compinit -u -d ./zcompdump && source ./_cupertino && print -r -- $_comps[cupertino]",
    );
    assert_eq!(registered, "_cupertino\n");
    // from fpath, as a package installs it; compinit skips a `.` entry, so the directory is absolute
    let autoloaded = shell(
        "zsh",
        dir.path(),
        "fpath=($PWD $fpath) && autoload -Uz compinit && compinit -u -d ./zcompdump2 && print -r -- $_comps[cupertino]",
    );
    assert_eq!(autoloaded, "_cupertino\n");
}

/// What the zsh script offers for `line`, the cursor at its end. Outside a completion widget `words` and `CURRENT` are
/// plain parameters and `compadd` a function of the test's: it keeps the candidates the current word begins, as zsh
/// does, and `_files` stands for the file completion.
fn zsh_offers(dir: &Path, registry: &[Group], line: &str) -> Vec<String> {
    std::fs::write(dir.join("_cupertino"), script("zsh", registry)).unwrap();
    let mut words: Vec<&str> = line.split_whitespace().collect();
    if line.ends_with(' ') {
        words.push("");
    }
    let words: Vec<String> = words.iter().map(|w| sh(w)).collect();
    let code = format!(
        "compdef() {{ :; }}\n\
         compadd() {{ local -a xs; xs=(${{(M)${{(P)2}}:#${{words[CURRENT]}}*}}); (( $#xs )) && print -rl -- $xs; return 0; }}\n\
         _files() {{ print -r -- '(files)'; }}\n\
         source ./_cupertino\n\
         words=({}) CURRENT={}\n\
         _cupertino",
        words.join(" "),
        words.len()
    );
    let mut offers: Vec<String> = shell("zsh", dir, &code)
        .lines()
        .map(str::to_owned)
        .collect();
    offers.sort();
    offers
}

#[test]
fn zsh_completes_commands_options_and_values() {
    if !have("zsh") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(zsh_offers(d, REGISTRY, "cupertino dia"), ["diagnose"]);
    assert_eq!(zsh_offers(d, REGISTRY, "cupertino de"), ["desktop", "dev"]);
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino --format json desk"),
        ["desktop"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino --format "),
        ["human", "json", "jsonl"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --area g"),
        ["gpu"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino desktop start --backend "),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --strict --"),
        ["--area", "--format", "--help", "--version"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino diagnose --area gpu --"),
        ["--area", "--format", "--help", "--strict", "--version"]
    );
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino help dev "),
        ["layers", "manual"]
    );
    assert_eq!(zsh_offers(d, REGISTRY, "cupertino he"), ["help"]);
    assert_eq!(
        zsh_offers(d, REGISTRY, "cupertino dev manual "),
        ["(files)"]
    );
    assert_eq!(
        zsh_offers(d, PROBE, "cupertino probe widget --colour "),
        ["blue", "green", "red"]
    );
    assert_eq!(
        zsh_offers(d, PROBE, "cupertino probe widget "),
        ["gear", "spring"]
    );
}

/// What fish offers for `line` with the script loaded.
fn fish_offers(dir: &Path, registry: &[Group], line: &str) -> Vec<String> {
    std::fs::write(dir.join("cupertino.fish"), script("fish", registry)).unwrap();
    let code = format!("source ./cupertino.fish; complete -C {}", sh(line));
    let mut offers: Vec<String> = shell("fish", dir, &code)
        .lines()
        .map(|l| l.split('\t').next().unwrap().to_owned())
        .collect();
    offers.sort();
    offers
}

#[test]
fn fish_completes_commands_options_and_values() {
    if !have("fish") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert_eq!(fish_offers(d, REGISTRY, "cupertino dia"), ["diagnose"]);
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino diagnose --area g"),
        ["gpu"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino desktop start --backend "),
        ["headless", "nested", "tty"]
    );
    assert_eq!(
        fish_offers(d, REGISTRY, "cupertino help dev "),
        ["layers", "manual"]
    );
    assert_eq!(
        fish_offers(d, PROBE, "cupertino probe widget --colour "),
        ["blue", "green", "red"]
    );
}
