# cupertino-build-info

What a Cupertino build is and what it runs on. Layer: `foundation`. Licence: MPL-2.0. It depends on nothing beyond
`std`, so every process, the toolkit and the C library can link it.

## Public API

| Item | What it gives |
|---|---|
| `COMMIT: Option<&str>` | The full hash of the commit the sources come from, in lower case, or `None` when the build could not know it. |
| `COMMIT_SOURCE: CommitSource` | Where `COMMIT` came from: `Environment`, `Archive`, `Checkout` or `Unknown` (`name()` gives the lower-case word). |
| `short_commit()` | The first 12 digits of `COMMIT`. |
| `Component { name, version }`, `component!()` | A program or library of the desktop. `component!()` takes the caller's `CARGO_PKG_NAME` and `CARGO_PKG_VERSION`; `component!("cupertino")` names a binary whose name differs from its package. Its `Display` is `cupertino 0.1.0 (0123456789ab)`, without the parenthesis when the commit is unknown. |
| `os_image()`, `read_os_image(root)`, `OsImage::parse(text)` | `NAME`, `ID`, `VERSION_ID`, `BUILD_ID`, `IMAGE_ID` and `IMAGE_VERSION` from `etc/os-release`, or `usr/lib/os-release` when the first does not exist, under a root. Every field is optional. `read_os_image` returns `NotFound` when neither file exists and the read error otherwise, so a caller can tell missing from unreadable; `os_image()` reads `/` and gives an empty `OsImage` on any error. |
| `kernel()`, `kernel_in(root)` | The running kernel's release from `proc/sys/kernel/osrelease`, or `None`. |

The parser follows the `os-release` format: blank lines and `#` comments are skipped, a value may be in double quotes
(with `\"`, `\\`, `\$` and `` \` `` escapes) or single quotes (literal), a backslash in an unquoted value stands for the
character after it, a later line for the same key wins, and an empty value counts as not set.

## Where the commit comes from

`build.rs` takes the first source that knows the commit (`build/commit.rs`; `tests/commit-source.rs` runs the order on
temporary trees):

1. `CUPERTINO_BUILD_COMMIT` from the environment, for a build that knows its commit otherwise. It must be a full
   hash, 40 hexadecimal digits (or 64 for a repository with sha256 object names); any other value fails the build,
   because it was set on purpose.
2. The tracked file `data/build-commit`. In the repository it holds `$Format:%H$`, and `.gitattributes` marks it
   `export-subst`, so `git archive` (and `packaging/release.py source`) writes the archived commit into it. A file with
   any other text is skipped with a Cargo warning.
3. `git rev-parse HEAD` when the workspace has a `.git` (a directory, or the file of a linked worktree).
4. Otherwise the commit is unknown, and `COMMIT` is `None`.

The script reruns only when `CUPERTINO_BUILD_COMMIT`, `data/build-commit` or the script itself change. A new commit in a
checkout does not rerun it, so it does not rebuild every binary that links this crate, and a shared build directory
stays reusable across commits. In a development build `COMMIT_SOURCE` is then `Checkout` and the value can be older
than the checkout's `HEAD`; `cargo clean -p cupertino-build-info` or `CUPERTINO_BUILD_COMMIT` gives the exact one.
Package builds start from a fresh source tree, the release archive (stable recipe) or a clone (development
recipe), and get the exact commit.

The commit does not affect reproducibility: it is a fixed input of a given source tree, like the sources themselves.
Two builds of the same archive or the same checkout `HEAD` embed the same value; nothing else here reads the build
machine (no time, host or path).
