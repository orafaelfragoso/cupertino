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
