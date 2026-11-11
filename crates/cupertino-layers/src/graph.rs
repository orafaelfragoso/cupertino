use crate::{allows, exception, Dep, Edge, Exception, Kind, Layer, Rules};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;

/// The committed DOT snapshot of the layers, relative to the workspace root.
pub const GRAPH_SNAPSHOT: &str = "crates/cupertino-layers/graph/layers.dot";

/// `graph` without its two count lines, which change with every dependency: a snapshot is compared without them.
pub fn without_counts(graph: &str) -> String {
    graph
        .lines()
        .filter(|l| {
            !(l.starts_with("// per layer: ")
                || l.starts_with("// ") && l.ends_with(" internal dependencies"))
        })
        .map(|l| format!("{l}\n"))
        .collect()
}

/// For every crate in a `denied` source layer, the shortest dependency chain to each crate of a layer it must not
/// reach, as crate names from source to target.
pub fn denied_paths(edges: &[Edge], denied: &[(Layer, &[Layer])]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for &(from_layer, to_layers) in denied {
        let starts: BTreeSet<&str> = edges
            .iter()
            .filter(|e| e.from_layer == from_layer)
            .map(|e| e.from.as_str())
            .collect();
        for start in starts {
            let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
            let mut queue = VecDeque::from([start]);
            while let Some(node) = queue.pop_front() {
                for e in edges.iter().filter(|e| e.from == node) {
                    if e.to == start || parent.contains_key(e.to.as_str()) {
                        continue;
                    }
                    parent.insert(&e.to, node);
                    if !to_layers.contains(&e.to_layer) {
                        queue.push_back(&e.to);
                        continue;
                    }
                    let mut path = vec![e.to.clone()];
                    let mut at = node;
                    while at != start {
                        path.push(at.to_owned());
                        at = parent[at];
                    }
                    path.push(start.to_owned());
                    path.reverse();
                    out.push(path);
                }
            }
        }
    }
    out
}

/// For every source crate in `denied`, the shortest chain of normal dependencies in `deps` to each crate it must not
/// reach, as crate names from source to target.
pub fn denied_crate_paths(deps: &BTreeSet<Dep>, denied: &[(&str, &[&str])]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    for &(start, targets) in denied {
        let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            for (_, to, _) in deps
                .iter()
                .filter(|(from, _, kind)| from == node && *kind == Kind::Normal)
            {
                if to == start || parent.contains_key(to.as_str()) {
                    continue;
                }
                parent.insert(to, node);
                if !targets.contains(&to.as_str()) {
                    queue.push_back(to);
                    continue;
                }
                let mut path = vec![to.clone()];
                let mut at = node;
                while at != start {
                    path.push(at.to_owned());
                    at = parent[at];
                }
                path.push(start.to_owned());
                path.reverse();
                out.push(path);
            }
        }
    }
    out
}

/// A DOT graph of `edges`, one cluster per layer, preceded by comment lines with the crate count per layer, every
/// edge `rules` forbid with its kind and exception status, stale exceptions and denied paths.
pub fn dot(
    title: &str,
    rules: Rules,
    layers: &BTreeMap<String, Layer>,
    edges: &[Edge],
    exceptions: &[Exception],
    denied: &[(Layer, &[Layer])],
) -> String {
    let mut by_layer: BTreeMap<Layer, Vec<&str>> = BTreeMap::new();
    for (name, layer) in layers {
        by_layer.entry(*layer).or_default().push(name);
    }
    let count = |l: &Layer| by_layer.get(l).map_or(0, Vec::len);
    let mut s = String::new();
    let _ = writeln!(s, "// cupertino-layers --graph");
    let _ = writeln!(
        s,
        "// {} crates, {} internal dependencies",
        layers.len(),
        edges.len()
    );
    let counts: Vec<String> = rules
        .iter()
        .map(|(l, _)| format!("{l} {}", count(l)))
        .collect();
    let _ = writeln!(s, "// per layer: {}", counts.join(", "));

    let forbidden: Vec<&Edge> = edges
        .iter()
        .filter(|e| !allows(rules, e.from_layer, e.to_layer))
        .collect();
    if forbidden.is_empty() {
        let _ = writeln!(s, "// forbidden edges: none");
    }
    for e in &forbidden {
        let status = match exception(exceptions, e) {
            Some(x) => format!("{} exception ({}): {}", x.tracking, x.area, x.reason),
            None => "violation".to_owned(),
        };
        let _ = writeln!(
            s,
            "// forbidden {} edge {} ({}) -> {} ({}), {status}",
            e.kind, e.from, e.from_layer, e.to, e.to_layer
        );
    }
    for x in exceptions {
        if !edges.iter().any(|e| x.matches(e)) {
            let _ = writeln!(s, "// stale {} exception {} -> {}", x.kind, x.from, x.to);
        }
    }
    for (from, to_layers) in denied {
        let to: Vec<&str> = to_layers.iter().map(|l| l.name()).collect();
        let _ = write!(s, "// denied paths {from} -> {}:", to.join(", "));
        let paths = denied_paths(edges, &[(*from, to_layers)]);
        if paths.is_empty() {
            s.push_str(" none");
        }
        s.push('\n');
        for p in paths {
            let _ = writeln!(s, "//   {}, violation", p.join(" -> "));
        }
    }

    let _ = writeln!(s, "digraph \"{title}\" {{");
    s.push_str("  node [shape=box];\n");
    for (layer, names) in &by_layer {
        let _ = writeln!(s, "  subgraph \"cluster_{layer}\" {{");
        let _ = writeln!(s, "    label=\"{layer} ({})\";", names.len());
        for name in names {
            let _ = writeln!(s, "    \"{name}\";");
        }
        s.push_str("  }\n");
    }
    for e in edges {
        let mut attrs = Vec::new();
        if e.kind != Kind::Normal {
            attrs.push(format!("label=\"{}\"", e.kind));
        }
        if !allows(rules, e.from_layer, e.to_layer) {
            match exception(exceptions, e) {
                Some(_) => attrs.push("color=orange, style=dashed".to_owned()),
                None => attrs.push("color=red".to_owned()),
            }
        }
        let attrs = if attrs.is_empty() {
            String::new()
        } else {
            format!(" [{}]", attrs.join(", "))
        };
        let _ = writeln!(s, "  \"{}\" -> \"{}\"{attrs};", e.from, e.to);
    }
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{edge, Area, Layer::*, Tracking};

    #[test]
    fn denied_paths_follow_dependency_chains() {
        let edges = [
            edge("kit", Ui, "net", Transport),
            edge("net", Transport, "server", Core),
            edge("kit", Ui, "geometry", Foundation),
        ];
        assert_eq!(
            denied_paths(&edges, &[(Ui, &[Core])]),
            vec![vec![
                "kit".to_owned(),
                "net".to_owned(),
                "server".to_owned()
            ]]
        );
    }

    #[test]
    fn denied_crate_paths_follow_normal_dependencies_to_external_crates() {
        let dep = |from: &str, to: &str, kind| (from.to_owned(), to.to_owned(), kind);
        let deps = BTreeSet::from([
            dep("transport", "geometry", Kind::Normal),
            dep("geometry", "wlr", Kind::Normal),
            dep("transport", "private", Kind::Dev),
            dep("other", "private", Kind::Normal),
        ]);
        assert_eq!(
            denied_crate_paths(&deps, &[("transport", &["wlr", "private"])]),
            vec![vec![
                "transport".to_owned(),
                "geometry".to_owned(),
                "wlr".to_owned()
            ]]
        );
    }

    #[test]
    fn dot_marks_exceptions_and_violations() {
        let layers = BTreeMap::from([
            ("files".to_owned(), Apps),
            ("glass".to_owned(), Render),
            ("theme".to_owned(), Foundation),
            ("wm".to_owned(), Models),
        ]);
        let edges = [
            edge("files", Apps, "glass", Render),
            edge("files", Apps, "theme", Foundation),
            Edge {
                kind: Kind::Dev,
                ..edge("wm", Models, "glass", Render)
            },
        ];
        let rules: Rules = &[
            (Foundation, &[Foundation]),
            (Models, &[Foundation]),
            (Render, &[Foundation]),
            (Apps, &[Foundation, Models]),
        ];
        let exception = |to, tracking| Exception {
            from: "files",
            to,
            kind: Kind::Normal,
            tracking,
            area: Area::Apps,
            reason: "until the toolkit",
        };
        let exceptions = [
            exception("glass", Tracking::Issue(46)),
            exception("wm", Tracking::Permanent),
        ];
        assert_eq!(
            dot(
                "t",
                rules,
                &layers,
                &edges,
                &exceptions,
                &[(Apps, &[Render])]
            ),
            r#"// cupertino-layers --graph
// 4 crates, 3 internal dependencies
// per layer: foundation 1, models 1, render 1, apps 1
// forbidden normal edge files (apps) -> glass (render), migration #46 exception (apps): until the toolkit
// forbidden dev edge wm (models) -> glass (render), violation
// stale normal exception files -> wm
// denied paths apps -> render:
//   files -> glass, violation
digraph "t" {
  node [shape=box];
  subgraph "cluster_foundation" {
    label="foundation (1)";
    "theme";
  }
  subgraph "cluster_models" {
    label="models (1)";
    "wm";
  }
  subgraph "cluster_render" {
    label="render (1)";
    "glass";
  }
  subgraph "cluster_apps" {
    label="apps (1)";
    "files";
  }
  "files" -> "glass" [color=orange, style=dashed];
  "files" -> "theme";
  "wm" -> "glass" [label="dev", color=red];
}
"#
        );
    }
}
