// Written by ci/test_suite.py from the files in this directory: every integration test of the crate in one
// program, so the crate and its dependencies are linked once. Add a test file, then run `ci/test_suite.py --write`.

#[allow(dead_code, unused_imports, unused_macros)]
mod support;

#[path = "assistive-technology.rs"]
mod assistive_technology;
#[path = "control-fixtures.rs"]
mod control_fixtures;
#[path = "damage-coverage.rs"]
mod damage_coverage;
#[path = "examples.rs"]
mod examples;
#[path = "table-header-atspi.rs"]
mod table_header_atspi;
#[path = "virtual-list-memory.rs"]
mod virtual_list_memory;
