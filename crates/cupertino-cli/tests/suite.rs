// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod support;

#[path = "cli.rs"]
mod cli;
#[path = "completions.rs"]
mod completions;
#[path = "desktop.rs"]
mod desktop;
#[path = "dev.rs"]
mod dev;
#[path = "dev-folder.rs"]
mod dev_folder;
#[path = "diagnose.rs"]
mod diagnose;
#[path = "diagnose-events.rs"]
mod diagnose_events;
#[path = "manual.rs"]
mod manual;
#[path = "registry.rs"]
mod registry;
#[path = "sdk.rs"]
mod sdk;
#[path = "shell.rs"]
mod shell;
#[path = "stream.rs"]
mod stream;
#[path = "stream-process.rs"]
mod stream_process;
#[path = "version.rs"]
mod version;
