//! The committed graph is compared without its two count lines (`// N crates, M internal dependencies` and
//! `// per layer: ...`). The merge queue regenerates them after each merge, so a branch that adds a crate or a
//! dependency does not commit them and two such branches do not conflict on them.

use std::process::Command;

use cupertino_layers::without_counts;

#[test]
fn committed_graph_matches_the_workspace() {
    let out = Command::new(env!("CARGO_BIN_EXE_cupertino-layers"))
        .arg("--graph")
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .output()
        .expect("run cupertino-layers");
    assert!(
        out.status.success(),
        "cupertino-layers --graph: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/graph/layers.dot");
    let committed = std::fs::read_to_string(path).expect("read snapshot");
    assert!(
        without_counts(&String::from_utf8(out.stdout).expect("UTF-8 graph"))
            == without_counts(&committed),
        "{path} is out of date: cargo run -q -p cupertino-layers -- --graph > crates/cupertino-layers/graph/layers.dot"
    );
}

#[test]
fn only_the_two_count_lines_are_left_out_of_the_comparison() {
    let graph = "// cupertino-layers --graph\n\
                 // 79 crates, 369 internal dependencies\n\
                 // per layer: foundation 18, tools 4\n\
                 // forbidden normal edge a (apps) -> b (render), migration #46 exception (apps): reason\n\
                 digraph layers {\n}\n";
    assert_eq!(
        without_counts(graph),
        "// cupertino-layers --graph\n\
         // forbidden normal edge a (apps) -> b (render), migration #46 exception (apps): reason\n\
         digraph layers {\n}\n"
    );
}
