"""Judge the `cargo test` log of the golden tests that `ci/goldens.sh` runs.

A comparison passes when exactly the golden tests ran and passed. An update re-records the baselines and then fails
each test that recorded (so it can never pass as a comparison); it passes when every golden test either passed or
failed with its re-record message alone, at least one recorded, and nothing failed for another reason. A test fails
with that message alone only when its worker's sole failure was the re-record (tests/goldens.rs, `only_re_recorded`);
anything else after it fails the test as `isolated golden worker failed`. No line is counted: a failing test's section
also holds the worker's own output, which repeats the re-record line.

Usage: python3 -I ci/goldens_log.py [--update] LOG
"""
import re
import sys
from pathlib import Path

# The tests of the server's test suite that ci/goldens.sh runs, by their path in it (the `goldens` module); change it
# with the tests.
TESTS = (
    "goldens::pixel_comparison_fails_on_a_change_at_unchanged_geometry",
    "goldens::only_a_lone_re_record_panic_is_an_update",
    "goldens::desktop_goldens_and_input_replay",
    "goldens::shell_process_goldens",
)

RESULT = re.compile(r"^test (\S+) \.\.\. (ok|FAILED)$")
SECTION = re.compile(r"^---- (\S+) stdout ----$")
PANIC = re.compile(r"^thread '(\S+)'(?: \([0-9]+\))? panicked at \S+:$")
RECORDED = re.compile(r"^re-recorded [0-9]+ goldens; rerun without CUPERTINO_UPDATE_GOLDENS$")


def results(log: str) -> dict[str, set[str]]:
    """Each test's outcomes; a test's worker repeats its result line, so a test can appear more than once."""
    found: dict[str, set[str]] = {}
    for line in log.splitlines():
        match = RESULT.match(line)
        if match:
            found.setdefault(match[1], set()).add(match[2])
    return found


def sections(log: str) -> dict[str, str]:
    """The captured output of each failed test, from its `---- name stdout ----` header to the next header. A section
    holds the test's worker output too, with that worker's own `failures:` lines, so those do not end it."""
    found: dict[str, list[str]] = {}
    current = None
    for line in log.splitlines():
        match = SECTION.match(line)
        if match:
            current = match[1]
            found.setdefault(current, [])
        elif current is not None:
            found[current].append(line)
    return {name: "\n".join(lines) for name, lines in found.items()}


def re_recorded(name: str, section: str) -> bool:
    """Whether the test failed with the re-record message alone: its own panic is the section's only unindented one
    (the worker's stderr is indented) and that panic's message is the re-record line."""
    lines = section.splitlines()
    panics = [i for i, line in enumerate(lines) if PANIC.match(line)]
    if len(panics) != 1 or PANIC.match(lines[panics[0]])[1] != name:
        return False
    following = lines[panics[0] + 1 : panics[0] + 2]
    return bool(following) and RECORDED.match(following[0]) is not None


def verdict(log: str, update: bool) -> str | None:
    """`None` when the log passes, else why not."""
    ran = results(log)
    unknown = sorted(set(ran) - set(TESTS))
    if unknown:
        return f"tests not in the golden list ran: {', '.join(unknown)}"
    missing = [name for name in TESTS if name not in ran]
    if missing:
        return f"expected tests did not run: {', '.join(missing)}"
    failed = [name for name in TESTS if "FAILED" in ran[name]]
    if not update:
        return f"tests failed: {', '.join(failed)}" if failed else None
    if not failed:
        return "an update re-recorded nothing"
    output = sections(log)
    other = [name for name in failed if not re_recorded(name, output.get(name, ""))]
    if other:
        return f"tests failed without re-recording: {', '.join(other)}"
    return None


def main(args: list[str]) -> int:
    update = args[:1] == ["--update"]
    paths = args[1:] if update else args
    if len(paths) != 1:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    problem = verdict(Path(paths[0]).read_text(errors="replace"), update)
    if problem:
        print(f"goldens: {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
