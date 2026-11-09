//! Manual pages generated from a registry: `cupertino.1` for the command, `cupertino-<group>.1` for each group.
//!
//! Everything a page says about a command comes from the registry (paths, arguments, descriptions, examples, the
//! stability); the general sections of `cupertino.1` (output forms, exit status, environment, files) come from the
//! registry's constants and [`crate::FailureKind::ALL`]. The pages are man(7) source. Their date is given, never the
//! current time, so two runs write the same bytes.

use crate::{
    Arg, ArgKind, Command, FailureKind, Group, Stability, BUILTINS, GLOBAL_OPTIONS, RESERVED,
    SCHEMA, STREAM_SCHEMA,
};
use std::ffi::OsStr;
use std::fmt::Write as _;

/// The date of pages built without `SOURCE_DATE_EPOCH`: a fixed one, so a build never stamps its own time.
pub const FIXED_DATE: &str = "1970-01-01";

/// The `.TH` date of the pages: `SOURCE_DATE_EPOCH` (seconds since 1970, UTC) as `YYYY-MM-DD`, else [`FIXED_DATE`].
pub fn page_date(source_date_epoch: Option<&OsStr>) -> String {
    source_date_epoch
        .and_then(OsStr::to_str)
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|s| *s >= 0)
        .map_or_else(
            || FIXED_DATE.to_owned(),
            |seconds| civil_date(seconds / 86_400),
        )
}

/// `YYYY-MM-DD` of a day number since 1970-01-01, in the proleptic Gregorian calendar.
fn civil_date(days: i64) -> String {
    // shift the era to start on 0000-03-01, so a leap day is the last day of its year
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Every page of `registry` as `(file name, man(7) source)`: `cupertino.1`, then one per group in registry order.
pub fn pages(registry: &[Group], date: &str) -> Vec<(String, String)> {
    let mut pages = vec![("cupertino.1".to_owned(), main_page(registry, date))];
    pages.extend(
        registry
            .iter()
            .map(|g| (format!("cupertino-{}.1", g.name), group_page(g, date))),
    );
    pages
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Text for a line of a page: backslashes and hyphens escaped, `quoted` spans in bold, a leading dot or quote made
/// literal and characters outside ASCII written as escapes.
fn text(s: &str) -> String {
    let mut out = String::new();
    let mut bold = false;
    for c in s.trim().chars() {
        match c {
            '\\' => out.push_str("\\e"),
            '-' => out.push_str("\\-"),
            '`' => {
                out.push_str(if bold { "\\fR" } else { "\\fB" });
                bold = !bold;
            }
            '\n' => out.push(' '),
            c if c.is_ascii() => out.push(c),
            c => {
                let _ = write!(out, "\\[u{:04X}]", c as u32);
            }
        }
    }
    if bold {
        out.push_str("\\fR");
    }
    if out.starts_with('.') || out.starts_with('\'') {
        out.insert_str(0, "\\&");
    }
    out
}

/// Words in bold, as a command line is shown.
fn bold(s: &str) -> String {
    format!("\\fB{}\\fR", text(s).replace('`', ""))
}

/// The paragraphs of a description, separated by `.PP`.
fn paragraphs(out: &mut String, description: &str) {
    for (i, paragraph) in description.split("\n\n").enumerate() {
        if i > 0 {
            out.push_str(".PP\n");
        }
        let _ = writeln!(out, "{}", text(paragraph));
    }
}

/// An argument as the synopsis shows it: `\fB\-\-area\fR \fIarea\fR`, values as `\fIgpu\fR|\fIlogs\fR`.
fn arg_spelling(a: &Arg) -> String {
    let value = if a.values.is_empty() {
        format!("\\fI{}\\fR", text(a.name))
    } else {
        a.values
            .iter()
            .map(|v| format!("\\fI{}\\fR", text(v)))
            .collect::<Vec<_>>()
            .join("|")
    };
    match a.kind {
        ArgKind::Positional => value,
        ArgKind::Option => format!("\\fB\\-\\-{}\\fR {value}", text(a.name)),
        ArgKind::Flag => format!("\\fB\\-\\-{}\\fR", text(a.name)),
    }
}

/// `\fBcupertino diagnose\fR [\fB\-\-area\fR ...]...`
fn synopsis(c: &Command) -> String {
    let mut line = bold(&format!("cupertino {}", c.name()));
    for a in c.args {
        let spelling = arg_spelling(a);
        let repeat = if a.repeat { "..." } else { "" };
        match a.kind {
            ArgKind::Positional => {
                let _ = write!(line, " {spelling}{repeat}");
            }
            ArgKind::Option | ArgKind::Flag => {
                let _ = write!(line, " [{spelling}]{repeat}");
            }
        }
    }
    line
}

/// One `.TP` entry per argument.
fn arguments(out: &mut String, args: &[Arg]) {
    for a in args {
        let repeat = if a.repeat { " (repeatable)" } else { "" };
        let _ = writeln!(out, ".TP\n{}\n{}{repeat}", arg_spelling(a), text(a.help));
    }
}

fn title(out: &mut String, name: &str, date: &str) {
    let _ = writeln!(
        out,
        ".TH {} 1 {date} \"cupertino {VERSION}\" \"Cupertino Manual\"",
        text(&name.to_uppercase())
    );
}

fn main_page(registry: &[Group], date: &str) -> String {
    let mut out = String::new();
    title(&mut out, "cupertino", date);
    out.push_str(".SH NAME\ncupertino \\- the command line of the Cupertino desktop\n");
    let _ = writeln!(
        out,
        ".SH SYNOPSIS\n{} [{}] \\fIcommand\\fR [\\fIarguments\\fR]\n.br\n{} {}\n.br\n{} \\fB\\-\\-version\\fR",
        bold("cupertino"),
        arg_spelling(&GLOBAL_OPTIONS[0]),
        bold("cupertino"),
        BUILTINS
            .iter()
            .map(|b| format!("\\fB{}\\fR [\\fIcommand\\fR]", text(b.name)))
            .collect::<Vec<_>>()
            .join(" | "),
        bold("cupertino"),
    );
    out.push_str(".SH DESCRIPTION\n");
    out.push_str(
        "\\fBcupertino\\fR is the command line of the desktop: it checks the machine, starts the desktop, reports \
         versions and helps develop it. Its commands are grouped; each group has a page of its own.\n",
    );
    for g in registry {
        let _ = writeln!(
            out,
            ".TP\n{}\n{} See \\fBcupertino\\-{}\\fR(1).",
            bold(g.name),
            text(g.summary),
            text(g.name)
        );
    }
    for b in BUILTINS {
        let _ = writeln!(out, ".TP\n{}\n{}.", bold(b.usage), text(b.summary));
    }
    out.push_str(".SH GLOBAL OPTIONS\nThese options may stand anywhere on the line before a \\fB\\-\\-\\fR.\n");
    arguments(&mut out, GLOBAL_OPTIONS);
    let _ = writeln!(
        out,
        ".SH OUTPUT\n\
         With \\fB\\-\\-format human\\fR, the default, the output goes to standard output, an error goes to standard \
         error as \\fBcupertino:\\fR \\fImessage\\fR and a warning as \\fBcupertino: warning:\\fR \\fImessage\\fR.\n\
         .PP\n\
         With \\fB\\-\\-format json\\fR, one line goes to standard output, a \\fB{}\\fR envelope with \\fBschema\\fR, \
         \\fBcommand\\fR, \\fBrequest_id\\fR, \\fBok\\fR, and \\fBdata\\fR on success or \\fBerror\\fR \
         (\\fBkind\\fR, \\fBcode\\fR, \\fBmessage\\fR) on failure. A failure may carry \\fBdata\\fR too.\n\
         .PP\n\
         With \\fB\\-\\-format jsonl\\fR, standard output carries \\fB{}\\fR records, one JSON object per line, each \
         with \\fBschema\\fR, \\fBcommand\\fR, \\fBrequest_id\\fR, \\fBseq\\fR (counting from 0 without a gap) and \
         \\fBtype\\fR: \\fBevent\\fR (\\fBevent\\fR, \\fBdata\\fR), \\fBerror\\fR (a problem the command continued \
         past), \\fBresult\\fR (\\fBdata\\fR) and \\fBend\\fR (\\fBok\\fR, \\fBexit_code\\fR, \\fBerror\\fR when it \
         failed, \\fBreason\\fR \\fBstopped\\fR when a stop ended a command that runs until stopped). There is \
         exactly one \\fBend\\fR, always last, except after \\fBdesktop start \\-\\-backend tty\\fR, which prints \
         \\fBstarting\\fR and then becomes the session, so that nothing follows.\n\
         .PP\n\
         In the JSON forms nothing goes to standard error. When the reader of standard output goes away the command \
         stops and exits 0. Within a schema, fields, \\fBkind\\fR and \\fBcode\\fR values and event names are only \
         added to; readers ignore what they do not know.",
        text(SCHEMA),
        text(STREAM_SCHEMA)
    );
    out.push_str(".SH EXIT STATUS\n.TP\n.B 0\nSuccess; also a closed standard output, and a command that runs until stopped when a stop ends it.\n");
    for kind in FailureKind::ALL {
        let _ = writeln!(
            out,
            ".TP\n\\fB{}\\fR (\\fB{}\\fR)\n{}",
            kind.exit_code(),
            text(kind.name()),
            text(kind.meaning())
        );
    }
    out.push_str(
        ".SH ENVIRONMENT\n\
         .TP\n.B CUPERTINO_SERVER\n\
         The compositor \\fBdesktop start\\fR runs, for development; else the \\fBcupertino\\-server\\fR beside \
         \\fBcupertino\\fR, else \\fI/usr/bin/cupertino\\-server\\fR.\n\
         .TP\n.B XDG_RUNTIME_DIR\n\
         Where the compositor puts its socket and its runtime file.\n\
         .TP\n.B WAYLAND_DISPLAY\n\
         The compositor \\fBversion\\fR reports, and with \\fBDISPLAY\\fR the desktop \\fBdesktop start \\-\\-backend \
         nested\\fR opens its window in.\n\
         .TP\n.B XDG_SESSION_TYPE\n\
         A graphical session, \\fBwayland\\fR or \\fBx11\\fR, refuses \\fBdesktop start \\-\\-backend tty\\fR.\n\
         .TP\n.B XDG_STATE_HOME\n\
         The logs \\fBdiagnose\\fR reads, in \\fI$XDG_STATE_HOME/cupertino\\fR (\\fI~/.local/state/cupertino\\fR \
         without it).\n\
         .TP\n.B SOURCE_DATE_EPOCH\n\
         The date \\fBdev manual\\fR writes into the pages.\n\
         .TP\n.B CARGO\n\
         The Cargo \\fBdev layers\\fR runs; else \\fBcargo\\fR on \\fBPATH\\fR, which no other command searches.\n",
    );
    out.push_str(
        ".SH FILES\n\
         .TP\n.I /usr/share/cupertino/build-info.json\n\
         The build record of the runtime package: its package name, version, commit and source date.\n\
         .TP\n.I /usr/share/cupertino/assets/build-info.json\n\
         The build record of the data package.\n\
         .TP\n.I $XDG_RUNTIME_DIR/cupertino/server\\-NAME.json\n\
         Written by the compositor of the Wayland display \\fINAME\\fR while it runs: its version, commit, process, \
         backend and renderer.\n",
    );
    let reserved = RESERVED
        .iter()
        .map(|r| format!("\\fB{}\\fR ({})", text(r.name), text(r.purpose)))
        .collect::<Vec<_>>()
        .join(" and ");
    let _ = writeln!(
        out,
        ".SH STABILITY\n\
         Every command is stable, preview or deprecated. A stable command's options, \\fBkind\\fR and \\fBcode\\fR \
         values, exit codes and data fields are only added to within a schema. A preview command may change or go \
         away in any release; every \\fBdev\\fR command is a preview. A deprecated command still works for one more \
         release and prints a warning naming its replacement. The group names {reserved} are kept for later \
         releases."
    );
    out.push_str(".SH SEE ALSO\n");
    let refs: Vec<String> = registry
        .iter()
        .map(|g| format!(".BR cupertino\\-{} (1)", text(g.name)))
        .collect();
    let _ = writeln!(out, "{}", refs.join(" ,\n"));
    out
}

fn group_page(g: &Group, date: &str) -> String {
    let mut out = String::new();
    let name = format!("cupertino-{}", g.name);
    title(&mut out, &name, date);
    let _ = writeln!(out, ".SH NAME\n{} \\- {}", text(&name), text(g.summary));
    out.push_str(".SH SYNOPSIS\n");
    let synopses: Vec<String> = g.commands().map(synopsis).collect();
    let _ = writeln!(out, "{}", synopses.join("\n.br\n"));
    out.push_str(".SH DESCRIPTION\n");
    paragraphs(&mut out, g.description);
    out.push_str(
        ".PP\nEvery command also takes the global options of \\fBcupertino\\fR(1), \
         \\fB\\-\\-format human\\fR|\\fBjson\\fR|\\fBjsonl\\fR among them.\n",
    );
    out.push_str(".SH COMMANDS\n");
    for c in g.commands() {
        let _ = writeln!(
            out,
            ".SS {}\n{}",
            text(&format!("cupertino {}", c.name())),
            synopsis(c)
        );
        match c.stability {
            Stability::Stable => {}
            Stability::Preview => {
                out.push_str(".PP\nA preview: its options and output may change in any release.\n")
            }
            Stability::Deprecated { replacement } => {
                let _ = writeln!(
                    out,
                    ".PP\nDeprecated: it will be removed in a later release; use {}.",
                    bold(replacement)
                );
            }
        }
        let _ = writeln!(out, ".PP\n{}", text(c.summary));
        out.push_str(".PP\n");
        paragraphs(&mut out, c.description);
        if !c.args.is_empty() {
            out.push_str(".PP\nArguments:\n.RS\n");
            arguments(&mut out, c.args);
            out.push_str(".RE\n");
        }
        out.push_str(".PP\nExamples:\n.RS\n");
        for (line, what) in c.examples {
            let _ = writeln!(out, ".TP\n{}\n{}", bold(line), text(what));
        }
        out.push_str(".RE\n");
    }
    out.push_str(".SH SEE ALSO\n.BR cupertino (1)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_date_is_the_source_date_or_a_fixed_one() {
        assert_eq!(page_date(Some(OsStr::new("0"))), "1970-01-01");
        assert_eq!(page_date(Some(OsStr::new("1762646400"))), "2025-11-09");
        assert_eq!(page_date(Some(OsStr::new("951782400"))), "2000-02-29");
        assert_eq!(page_date(Some(OsStr::new("4107542399"))), "2100-02-28");
        for bad in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("x")),
            Some(OsStr::new("-5")),
        ] {
            assert_eq!(page_date(bad), FIXED_DATE, "{bad:?}");
        }
    }

    #[test]
    fn text_escapes_what_roff_reads_as_requests_and_escapes() {
        assert_eq!(text(".hidden"), "\\&.hidden");
        assert_eq!(text("'quote"), "\\&'quote");
        assert_eq!(text("a\\b"), "a\\eb");
        assert_eq!(text("--area x-y"), "\\-\\-area x\\-y");
        assert_eq!(
            text("use `cupertino-diagnose` now"),
            "use \\fBcupertino\\-diagnose\\fR now"
        );
        assert_eq!(text("an … end"), "an \\[u2026] end");
        assert_eq!(text("`open"), "\\fBopen\\fR");
    }
}
