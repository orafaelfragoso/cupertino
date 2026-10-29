use std::process::Command;

#[test]
fn committed_graph_matches_the_workspace() {
    let out = Command::new(env!("CARGO_BIN_EXE_cupertino-layers"))
        .arg("--graph")
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
        String::from_utf8(out.stdout).expect("UTF-8 graph") == committed,
        "{path} is out of date: cargo run -q -p cupertino-layers -- --graph > crates/cupertino-layers/graph/layers.dot"
    );
}
