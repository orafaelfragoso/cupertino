//! The `cupertino` command.
//!
//! Every command is an entry of one registry, [`REGISTRY`]: a list of [`Group`]s whose [`Command`]s carry everything the
//! help, the JSON command tree, the shell completions and the manual pages are generated from (paths, summaries,
//! descriptions, examples, arguments with their value lists, repetition and value hints, and a [`Stability`]). The
//! global options are registry data too, [`GLOBAL_OPTIONS`].
//!
//! [`Runner`] matches a command line against a registry, runs the command and prints its result in the format the
//! line asks for:
//! - `--format human` (the default): text on stdout, errors on stderr as `cupertino: <message>`;
//! - `--format json`: one [`SCHEMA`] envelope line on stdout;
//! - `--format jsonl`: [`STREAM_SCHEMA`] records, one per line, ending with exactly one `end` record.
//!
//! A command never prints. A [`Run::Once`] command returns a [`Reply`]; a [`Run::Stream`] command reports events
//! through a [`Sink`] while it runs, then returns its final [`Reply`]. A [`Failure`] gives the exit code by its
//! [`FailureKind`]. The rules for streams (records, a closed output, signals) are in [`stream`].

mod appearance;
mod sdk;
pub mod stream;

pub use stream::{Closed, Sink, StopFlag};

use serde::{Serialize, Serializer};
use serde_json::value::RawValue;
use std::any::Any;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The version of the JSON envelope; a change to the envelope or to a command's `data` that is not additive raises it.
pub const SCHEMA: &str = "cupertino.cli.v1";

/// The version of the JSON lines records of `--format jsonl`; a change that is not additive raises it.
pub const STREAM_SCHEMA: &str = "cupertino.cli.stream.v1";

/// The command groups. A group's name is the first word of its commands' paths, and each name appears once.
pub const REGISTRY: &[Group] = &[appearance::GROUP, sdk::GROUP];

/// Group names kept for commands of a later release. `cupertino <name>` answers `unsupported`/`not_in_this_release`
/// (exit 8) instead of `unknown_command`, and no group of this release may use them.
pub const RESERVED: &[Reserved] = &[
    Reserved {
        name: "system",
        purpose: "system updates and recovery",
    },
    Reserved {
        name: "ai",
        purpose: "the assistant and voice services",
    },
];

/// The options every command line accepts. `--format` may stand anywhere before a `--`; `--help` and `--version` are
/// read by the runner, not by the commands.
pub const GLOBAL_OPTIONS: &[Arg] = &[
    Arg::option(
        "format",
        "how to print: text for people, one JSON envelope, or JSON lines as the command runs",
    )
    .values(&["human", "json", "jsonl"]),
    Arg::flag("help", "print the help of the commands named before it"),
    Arg::flag("version", "print the version of the command"),
];

/// A command group: the commands whose paths start with `name`, with the text of its manual page.
pub struct Group {
    pub name: &'static str,
    pub summary: &'static str,
    /// The body of the group's manual page: paragraphs separated by a blank line.
    pub description: &'static str,
    /// The command lists of the modules that make up the group, in order; usually one, `&[COMMANDS]`.
    pub commands: &'static [&'static [Command]],
}

impl Group {
    /// The group's commands in registry order.
    pub fn commands(&self) -> impl Iterator<Item = &'static Command> {
        self.commands.iter().flat_map(|list| list.iter())
    }
}

/// The commands the runner answers itself, without a [`Group`]: their names are taken, and the completions and manual
/// pages list them with the registry's commands.
pub const BUILTINS: &[Builtin] = &[Builtin {
    name: "help",
    usage: "help [<command>]",
    summary: "Prints the help of the commands whose path starts with the given words, or of every command",
}];

/// A command the runner answers itself.
pub struct Builtin {
    pub name: &'static str,
    /// `help [<command>]`, without the program name; `<command>` is one or more words of a command path.
    pub usage: &'static str,
    pub summary: &'static str,
}

/// A group name kept for a later release.
pub struct Reserved {
    pub name: &'static str,
    pub purpose: &'static str,
}

pub struct Command {
    /// The words that name the command, `["appearance", "assets", "check"]`. The first is its group's name.
    pub path: &'static [&'static str],
    /// One line for the help and the completions.
    pub summary: &'static str,
    /// The text of its manual page entry: paragraphs separated by a blank line.
    pub description: &'static str,
    /// Complete command lines starting with `cupertino`, each with what it does.
    pub examples: &'static [(&'static str, &'static str)],
    pub stability: Stability,
    pub args: &'static [Arg],
    pub run: Run,
}

impl Command {
    /// `appearance assets check`
    pub fn name(&self) -> String {
        self.path.join(" ")
    }

    /// `appearance assets check <dir>`, with options in brackets and `...` after a repeatable argument.
    pub fn usage(&self) -> String {
        let mut usage = self.name();
        for a in self.args {
            let _ = write!(usage, " {}", a.usage());
        }
        usage
    }
}

/// How a command runs; see [`stream`] for the stream rules.
#[derive(Clone, Copy)]
pub enum Run {
    /// Computes its whole result, then returns it.
    Once(fn(&Args, &Context) -> Result<Reply, Failure>),
    /// Reports events through the [`Sink`] while it runs, then returns its final result.
    Stream(fn(&Args, &Context, &mut Sink) -> Result<Reply, Failure>),
}

/// The promise a command makes about its command line and output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stability {
    /// Its options, `kind`/`code` values, exit codes and data fields are only added to within `cupertino.cli.v1`.
    Stable,
    /// It may change or go away in any release; every `dev` command is a preview.
    Preview,
    /// It still works as when it was stable and will be removed in a later release; `replacement` names what to use.
    /// A run prints a notice: a `cupertino: warning:` line on stderr in human form, an `error` record of kind
    /// `unsupported` and code `deprecated` in JSON lines.
    Deprecated { replacement: &'static str },
}

impl Stability {
    /// `stable`, `preview` or `deprecated`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Preview => "preview",
            Self::Deprecated { .. } => "deprecated",
        }
    }
}

impl Serialize for Stability {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

#[derive(Debug, Serialize)]
pub struct Arg {
    pub name: &'static str,
    pub help: &'static str,
    pub kind: ArgKind,
    /// The only values the argument takes, or empty for any value. The parser refuses another value with
    /// `invalid_usage`/`invalid_value`.
    pub values: &'static [&'static str],
    /// A repeatable option may be given several times; a repeatable positional argument, which must be the last,
    /// takes every remaining word. [`Args::values`] gives them all.
    pub repeat: bool,
    /// What kind of value a free value is, for the completions.
    pub hint: ValueHint,
}

impl Arg {
    /// A required argument that takes the next word that is not an option.
    pub const fn positional(name: &'static str, help: &'static str) -> Self {
        Self::new(name, help, ArgKind::Positional)
    }

    /// An optional `--name value` (or `--name=value`).
    pub const fn option(name: &'static str, help: &'static str) -> Self {
        Self::new(name, help, ArgKind::Option)
    }

    /// An optional `--name` without a value.
    pub const fn flag(name: &'static str, help: &'static str) -> Self {
        Self::new(name, help, ArgKind::Flag)
    }

    const fn new(name: &'static str, help: &'static str, kind: ArgKind) -> Self {
        Self {
            name,
            help,
            kind,
            values: &[],
            repeat: false,
            hint: ValueHint::None,
        }
    }

    /// Limits the argument to these values.
    pub const fn values(mut self, values: &'static [&'static str]) -> Self {
        self.values = values;
        self
    }

    /// Makes the argument repeatable.
    pub const fn repeat(mut self) -> Self {
        self.repeat = true;
        self
    }

    pub const fn hint(mut self, hint: ValueHint) -> Self {
        self.hint = hint;
        self
    }

    /// `<dir>`, `--area <area>`, `--format <human|json|jsonl>` or `--strict`, without brackets.
    pub fn spelling(&self) -> String {
        let value = if self.values.is_empty() {
            format!("<{}>", self.name)
        } else {
            format!("<{}>", self.values.join("|"))
        };
        match self.kind {
            ArgKind::Positional => value,
            ArgKind::Option => format!("--{} {value}", self.name),
            ArgKind::Flag => format!("--{}", self.name),
        }
    }

    /// The spelling as a usage line shows it: options in brackets, `...` after a repeatable argument.
    pub fn usage(&self) -> String {
        let spelling = self.spelling();
        let repeat = if self.repeat { "..." } else { "" };
        match self.kind {
            ArgKind::Positional => format!("{spelling}{repeat}"),
            ArgKind::Option | ArgKind::Flag => format!("[{spelling}]{repeat}"),
        }
    }
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

/// What a free value names, so the completions can offer the right candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueHint {
    /// Any text; nothing to complete.
    None,
    /// A file path.
    File,
    /// A directory path.
    Dir,
    /// The name of a program, completed from the executables on `PATH`.
    Command,
    /// A whole command line, completed as one.
    Shell,
}

/// The parsed arguments of a command, by [`Arg::name`].
#[derive(Debug, Default)]
pub struct Args {
    values: BTreeMap<&'static str, Vec<String>>,
    flags: Vec<&'static str>,
}

impl Args {
    /// The value of a positional argument or an option, the first one of a repeatable argument; always `Some` for a
    /// positional argument.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values(name).first().map(String::as_str)
    }

    /// Every value of the argument in command-line order; empty when it was not given.
    pub fn values(&self, name: &str) -> &[String] {
        self.values.get(name).map_or(&[], Vec::as_slice)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }
}

/// What a command returns: its data for the JSON forms and its text for people.
pub struct Reply {
    data: Box<RawValue>,
    human: String,
}

impl Reply {
    /// `data` is the `data` of the JSON envelope and of the `result` record; a stream command's data must be a JSON
    /// object, because the JSON envelope adds its `events` and `warnings` to it. `human` is printed as it is, so it
    /// ends with a line end unless it is empty.
    pub fn new(data: &impl Serialize, human: impl Into<String>) -> Self {
        Self {
            data: serde_json::value::to_raw_value(data).expect("command data serializes to JSON"),
            human: human.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// A stable snake_case identifier of the failure within its kind.
    pub code: &'static str,
    pub message: String,
    /// What the command found that explains the failure, for the JSON envelope's `data` beside its `error`.
    #[serde(skip)]
    data: Option<Box<RawValue>>,
}

impl Failure {
    pub fn new(kind: FailureKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Adds what the command found to the failure: the JSON envelope of a failure prints it as `data` beside `error`
    /// (the problems a check found, for example). The other forms do not print it; a stream reports them as events.
    pub fn with_data(mut self, data: &impl Serialize) -> Self {
        self.data =
            Some(serde_json::value::to_raw_value(data).expect("failure data serializes to JSON"));
        self
    }

    /// An `invalid_usage` failure.
    pub fn usage(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(FailureKind::InvalidUsage, code, message)
    }

    /// `temporary`/`interrupted`: what a finite stream returns when a stop ends it before it finished.
    pub fn interrupted() -> Self {
        Self::new(
            FailureKind::Temporary,
            "interrupted",
            "stopped before it finished",
        )
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
    /// Every kind, in the order of its exit code, for the exit status tables of the help and the manual pages.
    pub const ALL: [Self; 7] = [
        Self::InvalidUsage,
        Self::Unavailable,
        Self::Denied,
        Self::Validation,
        Self::Temporary,
        Self::TransactionFailed,
        Self::Unsupported,
    ];

    /// The `kind` of the JSON forms: `invalid_usage`, `unavailable`…
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidUsage => "invalid_usage",
            Self::Unavailable => "unavailable",
            Self::Denied => "denied",
            Self::Validation => "validation",
            Self::Temporary => "temporary",
            Self::TransactionFailed => "transaction_failed",
            Self::Unsupported => "unsupported",
        }
    }

    /// One sentence on what the kind means, for the exit status tables.
    pub fn meaning(self) -> &'static str {
        match self {
            Self::InvalidUsage => {
                "The command line is not valid: an unknown command or option, or a missing or wrong value."
            }
            Self::Unavailable => {
                "The backend or service the command needs is not running or cannot be reached."
            }
            Self::Denied => "The caller is not allowed to do this.",
            Self::Validation => "What the command checked or read is not valid.",
            Self::Temporary => {
                "A problem that may pass: the output cannot be written, or a stop came before the command finished."
            }
            Self::TransactionFailed => "A system or package transaction failed.",
            Self::Unsupported => "This release or this system does not do what was asked.",
        }
    }

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

/// The output form a command line asks for with `--format`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
    Jsonl,
}

/// What a command may read of the machine. The real command uses [`Context::system`]; a test gives a temporary root,
/// chosen variables and its own fixtures, so no command reads the real `HOME`, `/usr` or session.
pub struct Context<'a> {
    root: PathBuf,
    vars: Vars,
    current_dir: Option<PathBuf>,
    provided: Vec<&'a dyn Any>,
}

enum Vars {
    Process,
    Fixed(BTreeMap<OsString, OsString>),
}

impl<'a> Context<'a> {
    /// The running system: root `/`, the process environment and working directory.
    pub fn system() -> Self {
        Self {
            root: PathBuf::from("/"),
            vars: Vars::Process,
            current_dir: None,
            provided: Vec::new(),
        }
    }

    /// A machine under `root` with no environment variables, whose working directory is `root`.
    pub fn isolated(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            current_dir: Some(root.clone()),
            root,
            vars: Vars::Fixed(BTreeMap::new()),
            provided: Vec::new(),
        }
    }

    /// Sets a variable; on a [`Context::system`] context it first copies the process environment.
    pub fn with_var(mut self, name: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        if let Vars::Process = self.vars {
            self.vars = Vars::Fixed(std::env::vars_os().collect());
        }
        if let Vars::Fixed(vars) = &mut self.vars {
            vars.insert(name.into(), value.into());
        }
        self
    }

    pub fn with_current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    /// Hands a value to the commands, which find it by type with [`Context::provided`]: a command module's test seam
    /// (fixture sources, a fake clock) without a global.
    pub fn provide<T: Any>(mut self, value: &'a T) -> Self {
        self.provided.push(value);
        self
    }

    /// The directory installed files are read under: `/` for the real command.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// An absolute path of the running system under [`Context::root`]: `path("/usr/share/cupertino")`.
    pub fn path(&self, absolute: impl AsRef<Path>) -> PathBuf {
        let absolute = absolute.as_ref();
        self.root
            .join(absolute.strip_prefix("/").unwrap_or(absolute))
    }

    pub fn var(&self, name: &str) -> Option<OsString> {
        match &self.vars {
            Vars::Process => std::env::var_os(name),
            Vars::Fixed(vars) => vars.get(std::ffi::OsStr::new(name)).cloned(),
        }
    }

    pub fn current_dir(&self) -> io::Result<PathBuf> {
        match &self.current_dir {
            Some(dir) => Ok(dir.clone()),
            None => std::env::current_dir(),
        }
    }

    /// The last value of type `T` given with [`Context::provide`].
    pub fn provided<T: Any>(&self) -> Option<&T> {
        self.provided
            .iter()
            .rev()
            .find_map(|v| v.downcast_ref::<T>())
    }
}

/// Runs command lines against a registry.
pub struct Runner<'a> {
    registry: &'a [Group],
    context: Context<'a>,
    stop: &'a StopFlag,
}

static NEVER_STOPPED: StopFlag = StopFlag::new();

impl<'a> Runner<'a> {
    /// A runner of `registry` on [`Context::system`] whose streams are never asked to stop.
    pub fn new(registry: &'a [Group]) -> Self {
        Self {
            registry,
            context: Context::system(),
            stop: &NEVER_STOPPED,
        }
    }

    pub fn context(mut self, context: Context<'a>) -> Self {
        self.context = context;
        self
    }

    /// The flag that asks a running stream to stop; [`StopFlag::on_signals`] also sets it on SIGINT and SIGTERM.
    pub fn stop(mut self, stop: &'a StopFlag) -> Self {
        self.stop = stop;
        self
    }

    /// Runs the command line `args` (without the program name), writing to `out` and `err` as it goes, and returns
    /// the exit code. Output that cannot be written ends the command: a closed pipe exits 0 without another word, any
    /// other write error exits 6 (`temporary`/`output_failed`).
    pub fn run_to(
        &self,
        args: &[String],
        request_id: &str,
        out: &mut dyn Write,
        err: &mut dyn Write,
    ) -> u8 {
        self.run_inner(args, None, request_id, out, err)
    }

    /// [`Runner::run_to`] into memory.
    pub fn run(&self, args: &[String], request_id: &str) -> Output {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = self.run_to(args, request_id, &mut out, &mut err);
        Output {
            code,
            stdout: String::from_utf8_lossy(&out).into_owned(),
            stderr: String::from_utf8_lossy(&err).into_owned(),
        }
    }

    fn run_inner(
        &self,
        args: &[String],
        refused: Option<Failure>,
        request_id: &str,
        out: &mut dyn Write,
        err: &mut dyn Write,
    ) -> u8 {
        let (format, words) = split_globals(args);
        let format = match (format, refused) {
            (Ok(format), None) => format,
            (Ok(format), Some(failure)) => {
                return stream::finish(format, "", request_id, out, err, Err(failure), None)
            }
            (Err(failure), _) => {
                return stream::finish(Format::Human, "", request_id, out, err, Err(failure), None)
            }
        };
        match self.dispatch(&words) {
            Dispatched::Done(command, result) => {
                stream::finish(format, &command, request_id, out, err, result, None)
            }
            Dispatched::Command(command, args) => {
                let name = command.name();
                let mut lines = stream::Lines::new(format, &name, request_id, out, err);
                // the JSON envelope has no place for it; the help's `stability` says it there
                if let (Stability::Deprecated { replacement }, false) =
                    (command.stability, format == Format::Json)
                {
                    let _ = lines.warn(&Failure::new(
                        FailureKind::Unsupported,
                        "deprecated",
                        format!(
                            "`{name}` is deprecated and will be removed in a later release; use `{replacement}`"
                        ),
                    ));
                }
                match command.run {
                    Run::Once(run) => {
                        let result = run(&args, &self.context);
                        lines.finish(result, None)
                    }
                    Run::Stream(run) => {
                        self.stop.arm();
                        let mut sink = Sink::new(lines, self.stop);
                        let result = run(&args, &self.context, &mut sink);
                        sink.finish(result)
                    }
                }
            }
        }
    }

    fn commands(&self) -> impl Iterator<Item = &'static Command> + '_ {
        self.registry.iter().flat_map(Group::commands)
    }

    fn dispatch(&self, words: &[&str]) -> Dispatched {
        // the words before the first option or `--`, which name a command or a help topic
        let topic: Vec<&str> = words
            .iter()
            .copied()
            .take_while(|w| !w.starts_with('-'))
            .collect();
        let options = words.split(|w| *w == "--").next().unwrap_or(&[]);
        match words {
            [] => Dispatched::Done("help".into(), self.help(&[])),
            ["--version"] => Dispatched::Done("version".into(), Ok(version())),
            ["help", topic @ ..] => Dispatched::Done("help".into(), self.help(topic)),
            _ if options.contains(&"--help") => Dispatched::Done("help".into(), self.help(&topic)),
            _ => {
                // the longest path wins, so `diagnose events` is not `diagnose` with an argument `events`
                let found = self
                    .commands()
                    .filter(|c| words.starts_with(c.path))
                    .max_by_key(|c| c.path.len());
                match found {
                    Some(c) => match parse(c, &words[c.path.len()..]) {
                        Ok(args) => Dispatched::Command(c, args),
                        Err(failure) => Dispatched::Done(c.name(), Err(failure)),
                    },
                    None if topic.is_empty() => Dispatched::Done(
                        String::new(),
                        Err(Failure::usage(
                            "unknown_option",
                            format!("unknown option `{}`; see `cupertino help`", words[0]),
                        )),
                    ),
                    None => Dispatched::Done(topic.join(" "), Err(self.missing(&topic))),
                }
            }
        }
    }

    /// Why no command matches `topic`.
    fn missing(&self, topic: &[&str]) -> Failure {
        if let Some(reserved) = self.reserved(topic) {
            return reserved;
        }
        if self.commands().any(|c| c.path.starts_with(topic)) {
            return Failure::usage(
                "missing_subcommand",
                format!(
                    "`{}` needs a subcommand; see `cupertino help {0}`",
                    topic.join(" ")
                ),
            );
        }
        unknown(topic)
    }

    fn reserved(&self, topic: &[&str]) -> Option<Failure> {
        let first = *topic.first()?;
        let reserved = RESERVED.iter().find(|r| r.name == first)?;
        if self.registry.iter().any(|g| g.name == first) {
            return None;
        }
        Some(Failure::new(
            FailureKind::Unsupported,
            "not_in_this_release",
            format!(
                "`cupertino {first}` ({}) is not in this release",
                reserved.purpose
            ),
        ))
    }

    fn help(&self, topic: &[&str]) -> Result<Reply, Failure> {
        let matching: Vec<&Command> = self
            .commands()
            .filter(|c| c.path.starts_with(topic))
            .collect();
        if matching.is_empty() {
            return Err(self.reserved(topic).unwrap_or_else(|| unknown(topic)));
        }
        let mut human = format!(
            "usage: cupertino [{}] <command> [<arguments>]\n       \
             cupertino {}\n       cupertino --version\n\ncommands:\n",
            GLOBAL_OPTIONS[0].spelling(),
            BUILTINS[0].usage,
        );
        for c in &matching {
            let _ = write!(human, "  {}", c.usage());
            match c.stability {
                Stability::Stable => human.push('\n'),
                Stability::Preview => human.push_str("  (preview)\n"),
                Stability::Deprecated { replacement } => {
                    let _ = writeln!(human, "  (deprecated: use `{replacement}`)");
                }
            }
            let _ = writeln!(human, "      {}", c.summary);
            for a in c.args {
                let _ = writeln!(human, "      {}  {}", a.spelling(), a.help);
            }
            // the whole entry when the topic names the command itself
            if c.path == topic {
                for paragraph in c.description.split("\n\n") {
                    let _ = writeln!(human, "\n      {}", paragraph.trim());
                }
                human.push_str("\n      examples:\n");
                for (line, what) in c.examples {
                    let _ = writeln!(human, "        {line}\n            {what}");
                }
            }
        }
        let entries: Vec<HelpEntry> = matching
            .iter()
            .map(|c| HelpEntry {
                path: c.name(),
                summary: c.summary,
                args: c.args,
                stability: c.stability,
                replacement: match c.stability {
                    Stability::Deprecated { replacement } => Some(replacement),
                    _ => None,
                },
                streams: matches!(c.run, Run::Stream(_)),
                description: c.description,
                examples: c
                    .examples
                    .iter()
                    .map(|&(command, description)| HelpExample {
                        command,
                        description,
                    })
                    .collect(),
            })
            .collect();
        Ok(Reply::new(&entries, human))
    }
}

enum Dispatched {
    /// Answered without running a command: help, the version or a usage error.
    Done(String, Result<Reply, Failure>),
    Command(&'static Command, Args),
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
    stability: Stability,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement: Option<&'static str>,
    streams: bool,
    description: &'static str,
    examples: Vec<HelpExample>,
}

#[derive(Serialize)]
struct HelpExample {
    command: &'static str,
    description: &'static str,
}

/// A request identifier unique on this machine: the process id and the time.
pub fn request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{:x}-{nanos:x}", std::process::id())
}

/// Runs the command line `args` (without the program name) against [`REGISTRY`] on the running system and returns
/// what it printed. `--format` may stand anywhere before a `--`.
pub fn run(args: &[String], request_id: &str) -> Output {
    Runner::new(REGISTRY).run(args, request_id)
}

/// Runs the command line `args` against [`REGISTRY`] on the running system, writing to `out` and `err` as it goes;
/// `stop` asks a running stream to stop. Returns the exit code.
pub fn run_to(
    args: &[String],
    request_id: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
    stop: &StopFlag,
) -> u8 {
    Runner::new(REGISTRY)
        .stop(stop)
        .run_to(args, request_id, out, err)
}

/// The whole program for `registry`: the process arguments, stdout and stderr, and a stop flag that SIGINT and SIGTERM
/// set while a stream runs. An argument that is not UTF-8 is `invalid_usage`/`invalid_encoding`.
pub fn main_with(registry: &[Group]) -> std::process::ExitCode {
    static STOP: StopFlag = StopFlag::on_signals();
    let mut refused = None;
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .enumerate()
        .map(|(i, a)| {
            a.into_string().unwrap_or_else(|_| {
                refused.get_or_insert_with(|| {
                    Failure::usage(
                        "invalid_encoding",
                        format!("argument {} is not UTF-8", i + 1),
                    )
                });
                String::new()
            })
        })
        .collect();
    let runner = Runner::new(registry).stop(&STOP);
    let (stdout, stderr) = (io::stdout(), io::stderr());
    let code = runner.run_inner(
        &args,
        refused,
        &request_id(),
        &mut stdout.lock(),
        &mut stderr.lock(),
    );
    std::process::ExitCode::from(code)
}

/// The `--format` value and the other words. Words after a `--` are the command's, `--format` included.
fn split_globals(args: &[String]) -> (Result<Format, Failure>, Vec<&str>) {
    let mut format = Ok(Format::Human);
    let mut words = Vec::new();
    let mut rest = args.iter().map(String::as_str);
    while let Some(word) = rest.next() {
        if word == "--" {
            words.push(word);
            words.extend(rest.by_ref());
            break;
        }
        let value = match word.strip_prefix("--format") {
            Some("") => rest.next(),
            Some(v) if v.starts_with('=') => Some(&v[1..]),
            _ => {
                words.push(word);
                continue;
            }
        };
        let choices = GLOBAL_OPTIONS[0].values;
        format = match value {
            Some("human") => Ok(Format::Human),
            Some("json") => Ok(Format::Json),
            Some("jsonl") => Ok(Format::Jsonl),
            Some(other) => Err(Failure::usage(
                "invalid_format",
                format!("--format takes {}, not `{other}`", one_of(choices)),
            )),
            None => Err(Failure::usage("missing_value", "--format needs a value")),
        };
    }
    (format, words)
}

/// `a`, `a or b`, `a, b or c`
fn one_of(values: &[&str]) -> String {
    match values {
        [] => String::new(),
        [one] => (*one).into(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
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
    let path = command.name();
    let mut args = Args::default();
    let mut positionals = command
        .args
        .iter()
        .filter(|a| a.kind == ArgKind::Positional)
        .peekable();
    let mut only_positionals = false;
    let mut rest = words.iter();
    while let Some(&word) = rest.next() {
        if word == "--" && !only_positionals {
            only_positionals = true;
            continue;
        }
        match word.strip_prefix("--").filter(|_| !only_positionals) {
            Some(spelled) => {
                let (name, inline) = match spelled.split_once('=') {
                    Some((name, value)) => (name, Some(value)),
                    None => (spelled, None),
                };
                let arg = command
                    .args
                    .iter()
                    .find(|a| a.kind != ArgKind::Positional && a.name == name)
                    .ok_or_else(|| {
                        Failure::usage("unknown_option", format!("`{path}` has no option --{name}"))
                    })?;
                if arg.kind == ArgKind::Flag {
                    if inline.is_some() {
                        return Err(Failure::usage(
                            "unexpected_value",
                            format!("--{name} takes no value"),
                        ));
                    }
                    if !args.flags.contains(&arg.name) {
                        args.flags.push(arg.name);
                    }
                    continue;
                }
                let value = match inline {
                    Some(value) => value,
                    None => rest.next().copied().ok_or_else(|| {
                        Failure::usage("missing_value", format!("--{name} needs a value"))
                    })?,
                };
                if !arg.repeat && args.values.contains_key(arg.name) {
                    return Err(Failure::usage(
                        "repeated_option",
                        format!("--{name} is given more than once"),
                    ));
                }
                take(&mut args, arg, value)?;
            }
            None => {
                let arg = match positionals.peek() {
                    Some(arg) if arg.repeat => *arg,
                    Some(_) => positionals.next().expect("peeked"),
                    None => {
                        return Err(Failure::usage(
                            "unexpected_argument",
                            format!("`{path}` takes no argument `{word}`"),
                        ))
                    }
                };
                take(&mut args, arg, word)?;
            }
        }
    }
    match positionals.find(|a| !args.values.contains_key(a.name)) {
        Some(missing) => Err(Failure::usage(
            "missing_argument",
            format!("`{path}` needs <{}>", missing.name),
        )),
        None => Ok(args),
    }
}

/// Adds `value` to the argument after checking it against its value list.
fn take(args: &mut Args, arg: &'static Arg, value: &str) -> Result<(), Failure> {
    if !arg.values.is_empty() && !arg.values.contains(&value) {
        let name = match arg.kind {
            ArgKind::Positional => format!("<{}>", arg.name),
            _ => format!("--{}", arg.name),
        };
        return Err(Failure::usage(
            "invalid_value",
            format!("{name} takes {}, not `{value}`", one_of(arg.values)),
        ));
    }
    args.values.entry(arg.name).or_default().push(value.into());
    Ok(())
}

fn version() -> Reply {
    let cli = env!("CARGO_PKG_VERSION");
    Reply::new(&Version { cli }, format!("cupertino {cli}\n"))
}

/// The registry rules every [`REGISTRY`] (and every test registry) keeps; one line per broken rule, empty when it keeps
/// them all:
/// - a group has a name, a summary and a description; names are unique, not [`RESERVED`] and not a [`BUILTINS`] name;
/// - a command's first word is its group's name; paths are unique; a command whose path begins another's takes no
///   positional argument, so the longer path is never read as an argument;
/// - a command has a summary, a description and at least one example, each a `cupertino` line naming the command;
/// - every `dev` command is a [`Stability::Preview`];
/// - argument names are unique per command and differ from the [`GLOBAL_OPTIONS`];
/// - a value list has no empty, repeated or spaced value; a flag has no values, repetition or hint, and an argument
///   with values no hint;
/// - a repeatable positional argument is the last positional one.
pub fn check_registry(registry: &[Group]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut names = Vec::new();
    let mut paths: Vec<&[&str]> = Vec::new();
    for group in registry {
        let g = group.name;
        if g.is_empty() || group.summary.is_empty() || group.description.is_empty() {
            problems.push(format!(
                "group `{g}` needs a name, a summary and a description"
            ));
        }
        if names.contains(&g) {
            problems.push(format!("group `{g}` is registered twice"));
        }
        names.push(g);
        if RESERVED.iter().any(|r| r.name == g) {
            problems.push(format!("group `{g}` is reserved for a later release"));
        }
        if BUILTINS.iter().any(|b| b.name == g) {
            problems.push(format!(
                "group `{g}` has the name of a command the runner answers itself"
            ));
        }
        for c in group.commands() {
            let name = c.name();
            if c.path.first() != Some(&g)
                || c.path.iter().any(|w| w.is_empty() || w.starts_with('-'))
            {
                problems.push(format!("`{name}` is not a path under its group `{g}`"));
            }
            if paths.contains(&c.path) {
                problems.push(format!("`{name}` is registered twice"));
            }
            paths.push(c.path);
            if c.summary.is_empty() || c.description.is_empty() || c.examples.is_empty() {
                problems.push(format!(
                    "`{name}` needs a summary, a description and an example"
                ));
            }
            for (line, what) in c.examples {
                let words: Vec<&str> = line.split_whitespace().collect();
                let mut words = words.as_slice();
                if let ["cupertino", rest @ ..] = words {
                    words = rest;
                    while let ["--format", _, rest @ ..] = words {
                        words = rest;
                    }
                    if words.starts_with(c.path) && !what.is_empty() {
                        continue;
                    }
                }
                problems.push(format!(
                    "`{name}` has an example that is not a `cupertino {name}` line with what it does: `{line}`"
                ));
            }
            if g == "dev" && c.stability != Stability::Preview {
                problems.push(format!("`{name}` is a dev command, so a preview"));
            }
            let mut seen: Vec<&str> = Vec::new();
            let positionals: Vec<&Arg> = c
                .args
                .iter()
                .filter(|a| a.kind == ArgKind::Positional)
                .collect();
            for a in c.args {
                let n = a.name;
                if n.is_empty() || a.help.is_empty() {
                    problems.push(format!("`{name}` has an argument without a name or help"));
                }
                if seen.contains(&n) || GLOBAL_OPTIONS.iter().any(|o| o.name == n) {
                    problems.push(format!("`{name}` has the argument name `{n}` twice"));
                }
                seen.push(n);
                if a.values.iter().enumerate().any(|(i, v)| {
                    v.is_empty() || v.contains(char::is_whitespace) || a.values[..i].contains(v)
                }) {
                    problems.push(format!(
                        "`{name}` `{n}` has an empty, spaced or repeated value"
                    ));
                }
                if a.kind == ArgKind::Flag
                    && (!a.values.is_empty() || a.repeat || a.hint != ValueHint::None)
                {
                    problems.push(format!(
                        "`{name}` flag `{n}` takes no value, so no values, repetition or hint"
                    ));
                }
                if !a.values.is_empty() && a.hint != ValueHint::None {
                    problems.push(format!("`{name}` `{n}` has values, so no hint"));
                }
            }
            if positionals.iter().rev().skip(1).any(|a| a.repeat) {
                problems.push(format!(
                    "`{name}` has a repeatable positional argument before another"
                ));
            }
        }
    }
    for c in registry.iter().flat_map(Group::commands) {
        let longer = paths
            .iter()
            .any(|p| p.len() > c.path.len() && p.starts_with(c.path));
        if longer && c.args.iter().any(|a| a.kind == ArgKind::Positional) {
            problems.push(format!(
                "`{}` begins a longer command path, so it takes no positional argument",
                c.name()
            ));
        }
    }
    problems
}
