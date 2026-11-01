//! The `cupertino` command. Each group module lists its subcommands as [`Command`]s in a `COMMANDS` constant;
//! [`run`] matches the arguments against [`REGISTRY`], runs the command and formats its [`Reply`] or [`Failure`] for
//! `--format human` (output to stdout, errors to stderr) or `--format json` (one [`SCHEMA`] envelope on stdout).

mod appearance;

use serde::Serialize;
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fmt::Write;

/// The version of the JSON envelope; a change to the envelope or to a command's `data` that is not additive raises it.
pub const SCHEMA: &str = "cupertino.cli.v1";

/// The command groups; a group is the first word of its commands' paths.
pub const REGISTRY: &[&[Command]] = &[appearance::COMMANDS];

pub struct Command {
    /// The words that name the command, `["appearance", "assets", "check"]`.
    pub path: &'static [&'static str],
    pub summary: &'static str,
    pub args: &'static [Arg],
    pub run: fn(&Args) -> Result<Reply, Failure>,
}

#[derive(Debug, Serialize)]
pub struct Arg {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: ArgKind,
}

/// A positional argument is required and takes the next word that is not an option; `--name value` options and
/// `--name` flags are optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArgKind {
    Positional,
    Option,
    Flag,
}

/// The parsed arguments of a command, by [`Arg::name`].
#[derive(Debug, Default)]
pub struct Args {
    values: BTreeMap<&'static str, String>,
    flags: Vec<&'static str>,
}

impl Args {
    /// The value of a positional argument or an option; always `Some` for a positional one.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }
}

/// What a command returns: its data for `--format json` and its text for `--format human`.
pub struct Reply {
    data: Box<RawValue>,
    human: String,
}

impl Reply {
    pub fn new(data: &impl Serialize, human: impl Into<String>) -> Self {
        Self {
            data: serde_json::value::to_raw_value(data).expect("command data serializes to JSON"),
            human: human.into(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// A stable snake_case identifier of the failure within its kind.
    pub code: &'static str,
    pub message: String,
}

impl Failure {
    pub fn new(kind: FailureKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    fn usage(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(FailureKind::InvalidUsage, code, message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    InvalidUsage,
    /// The backend or service the command needs is not running or cannot be reached.
    Unavailable,
    Denied,
    Validation,
    Temporary,
    /// A system or package transaction failed.
    TransactionFailed,
    Unsupported,
}

impl FailureKind {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::InvalidUsage => 2,
            Self::Unavailable => 3,
            Self::Denied => 4,
            Self::Validation => 5,
            Self::Temporary => 6,
            Self::TransactionFailed => 7,
            Self::Unsupported => 8,
        }
    }
}

/// What the process prints and its exit status.
#[derive(Debug, PartialEq, Eq)]
pub struct Output {
    pub code: u8,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Human,
    Json,
}

#[derive(Serialize)]
struct Envelope<'a> {
    schema: &'static str,
    command: &'a str,
    request_id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a Failure>,
}

#[derive(Serialize)]
struct Version {
    cli: &'static str,
}

#[derive(Serialize)]
struct HelpEntry {
    path: String,
    summary: &'static str,
    args: &'static [Arg],
}

/// A request identifier unique on this machine: the process id and the time.
pub fn request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}-{nanos:x}", std::process::id())
}

/// Runs the command line `args` (without the program name). `--format` may stand anywhere.
pub fn run(args: &[String], request_id: &str) -> Output {
    let mut format = Ok(Format::Human);
    let mut words = Vec::new();
    let mut rest = args.iter().map(String::as_str);
    while let Some(word) = rest.next() {
        let value = match word.strip_prefix("--format") {
            Some("") => rest.next(),
            Some(v) if v.starts_with('=') => Some(&v[1..]),
            _ => {
                words.push(word);
                continue;
            }
        };
        format = match value {
            Some("human") => Ok(Format::Human),
            Some("json") => Ok(Format::Json),
            Some(other) => Err(Failure::usage(
                "invalid_format",
                format!("--format takes human or json, not `{other}`"),
            )),
            None => Err(Failure::usage("missing_value", "--format needs a value")),
        };
    }
    match format {
        Ok(format) => {
            let (command, result) = dispatch(&words);
            render(format, &command, request_id, result)
        }
        Err(failure) => render(Format::Human, "", request_id, Err(failure)),
    }
}

fn dispatch(words: &[&str]) -> (String, Result<Reply, Failure>) {
    let topic: Vec<&str> = words
        .iter()
        .copied()
        .take_while(|w| !w.starts_with('-'))
        .collect();
    match words {
        [] => ("help".into(), help(&[])),
        ["--version"] => ("version".into(), Ok(version())),
        ["help", topic @ ..] => ("help".into(), help(topic)),
        _ if words.contains(&"--help") => ("help".into(), help(&topic)),
        _ => match commands().find(|c| words.starts_with(c.path)) {
            Some(c) => (
                c.path.join(" "),
                parse(c, &words[c.path.len()..]).and_then(|args| (c.run)(&args)),
            ),
            None if commands().any(|c| c.path.starts_with(&topic)) => (
                topic.join(" "),
                Err(Failure::usage(
                    "missing_subcommand",
                    format!(
                        "`{}` needs a subcommand; see `cupertino help {0}`",
                        topic.join(" ")
                    ),
                )),
            ),
            None => (topic.join(" "), Err(unknown(&topic))),
        },
    }
}

fn commands() -> impl Iterator<Item = &'static Command> {
    REGISTRY.iter().flat_map(|group| group.iter())
}

fn unknown(topic: &[&str]) -> Failure {
    Failure::usage(
        "unknown_command",
        format!(
            "unknown command `{}`; see `cupertino help`",
            topic.join(" ")
        ),
    )
}

fn parse(command: &Command, words: &[&str]) -> Result<Args, Failure> {
    let path = command.path.join(" ");
    let mut args = Args::default();
    let mut positionals = command
        .args
        .iter()
        .filter(|a| a.kind == ArgKind::Positional);
    let mut rest = words.iter();
    while let Some(&word) = rest.next() {
        if let Some(name) = word.strip_prefix("--") {
            let arg = command
                .args
                .iter()
                .find(|a| a.kind != ArgKind::Positional && a.name == name)
                .ok_or_else(|| {
                    Failure::usage("unknown_option", format!("`{path}` has no option --{name}"))
                })?;
            if arg.kind == ArgKind::Flag {
                args.flags.push(arg.name);
            } else {
                let value = rest.next().ok_or_else(|| {
                    Failure::usage("missing_value", format!("--{name} needs a value"))
                })?;
                args.values.insert(arg.name, (*value).to_string());
            }
        } else {
            let arg = positionals.next().ok_or_else(|| {
                Failure::usage(
                    "unexpected_argument",
                    format!("`{path}` takes no argument `{word}`"),
                )
            })?;
            args.values.insert(arg.name, word.to_string());
        }
    }
    match positionals.next() {
        Some(missing) => Err(Failure::usage(
            "missing_argument",
            format!("`{path}` needs <{}>", missing.name),
        )),
        None => Ok(args),
    }
}

fn version() -> Reply {
    let cli = env!("CARGO_PKG_VERSION");
    Reply::new(&Version { cli }, format!("cupertino {cli}\n"))
}

fn help(topic: &[&str]) -> Result<Reply, Failure> {
    let matching: Vec<&Command> = commands().filter(|c| c.path.starts_with(topic)).collect();
    if matching.is_empty() {
        return Err(unknown(topic));
    }
    let mut human = String::from(
        "usage: cupertino [--format human|json] <command> [<arguments>]\n       \
         cupertino help [<command>]\n       cupertino --version\n\ncommands:\n",
    );
    for c in &matching {
        let _ = writeln!(human, "  {}", usage(c));
        let _ = writeln!(human, "      {}", c.summary);
        for a in c.args {
            let _ = writeln!(human, "      {}  {}", spelling(a), a.help);
        }
    }
    let entries: Vec<HelpEntry> = matching
        .iter()
        .map(|c| HelpEntry {
            path: c.path.join(" "),
            summary: c.summary,
            args: c.args,
        })
        .collect();
    Ok(Reply::new(&entries, human))
}

fn usage(command: &Command) -> String {
    let mut usage = command.path.join(" ");
    for a in command.args {
        match a.kind {
            ArgKind::Positional => {
                let _ = write!(usage, " {}", spelling(a));
            }
            ArgKind::Option | ArgKind::Flag => {
                let _ = write!(usage, " [{}]", spelling(a));
            }
        }
    }
    usage
}

fn spelling(arg: &Arg) -> String {
    match arg.kind {
        ArgKind::Positional => format!("<{}>", arg.name),
        ArgKind::Option => format!("--{0} <{0}>", arg.name),
        ArgKind::Flag => format!("--{}", arg.name),
    }
}

fn render(
    format: Format,
    command: &str,
    request_id: &str,
    result: Result<Reply, Failure>,
) -> Output {
    let code = result.as_ref().map_or_else(|f| f.kind.exit_code(), |_| 0);
    match format {
        Format::Human => match result {
            Ok(reply) => Output {
                code,
                stdout: reply.human,
                stderr: String::new(),
            },
            Err(failure) => Output {
                code,
                stdout: String::new(),
                stderr: format!("cupertino: {}\n", failure.message),
            },
        },
        Format::Json => {
            let envelope = Envelope {
                schema: SCHEMA,
                command,
                request_id,
                ok: result.is_ok(),
                data: result.as_ref().ok().map(|r| r.data.as_ref()),
                error: result.as_ref().err(),
            };
            let mut stdout = serde_json::to_string(&envelope).expect("the envelope serializes");
            stdout.push('\n');
            Output {
                code,
                stdout,
                stderr: String::new(),
            }
        }
    }
}
