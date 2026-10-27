use std::process::Command;

fn graph(view: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_cupertino-layers"))
        .args(["--graph", view])
        .output()
        .expect("run cupertino-layers");
    assert!(
        out.status.success(),
        "cupertino-layers --graph {view}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("UTF-8 graph")
}

#[test]
fn committed_graphs_match_the_workspace() {
    for view in ["current", "target"] {
        let path = format!("{}/graph/{view}.dot", env!("CARGO_MANIFEST_DIR"));
        let committed = std::fs::read_to_string(&path).expect("read snapshot");
        assert!(
            graph(view) == committed,
            "{path} is out of date: cargo run -q -p cupertino-layers -- --graph {view} > crates/cupertino-layers/graph/{view}.dot"
        );
    }
}
