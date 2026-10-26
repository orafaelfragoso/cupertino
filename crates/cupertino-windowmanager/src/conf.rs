//! `windowmanager.conf`: `key = value` lines, `# starts a comment.
//!
//! | Key | Values | Default |
//! |---|---|---|
//! | `stage` | `on`, `off` | `off` (the design ships with the stage off) |

/// Settings read from `windowmanager.conf`; `None` for a key the file does not set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowManagerFile {
    pub stage: Option<bool>,
}

/// A line that was not applied: its 1-based number and what is wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfProblem {
    pub line: usize,
    pub problem: String,
}

/// Parse the file. Lines that are not understood are skipped and reported, so one typo does not
/// cost the other settings.
pub fn parse_conf(text: &str) -> (WindowManagerFile, Vec<ConfProblem>) {
    let mut file = WindowManagerFile::default();
    let mut problems = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut problem = |p: String| {
            problems.push(ConfProblem {
                line: i + 1,
                problem: p,
            })
        };
        let Some((key, value)) = line.split_once('=') else {
            problem(format!("{line:?} is not `key = value`"));
            continue;
        };
        match (key.trim(), value.trim()) {
            ("stage", "on") => file.stage = Some(true),
            ("stage", "off") => file.stage = Some(false),
            ("stage", other) => problem(format!("stage must be on or off, not {other:?}")),
            (other, _) => problem(format!("unknown key {other:?}")),
        }
    }
    (file, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_is_read_and_bad_lines_are_reported_without_losing_the_rest() {
        let (file, problems) =
            parse_conf("# settings\nstage = on  # turned on\nstage = maybe\nzoom = 3\nnonsense\n");
        assert_eq!(file.stage, Some(true));
        assert_eq!(
            problems.iter().map(|p| p.line).collect::<Vec<_>>(),
            vec![3, 4, 5]
        );
        assert_eq!(parse_conf("stage=off").0.stage, Some(false));
        assert_eq!(parse_conf("").0, WindowManagerFile::default());
    }
}
