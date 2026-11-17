use crate::{allows_edge, matches, Edge, Kind, Layer, Problem, Rules};
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

/// Follows `parent` back from `end` to `start`: the chain from `start` to `end`.
fn path_to(parent: &BTreeMap<&str, &str>, start: &str, end: &str) -> Vec<String> {
    let mut path = vec![end.to_owned()];
    let mut at = end;
    while at != start {
        at = parent[at];
        path.push(at.to_owned());
    }
    path.reverse();
    path
}

/// A problem for every crate that a workspace crate reaches through a chain of two or more dependencies and that sits
/// in a higher layer, `testing` aside: the crate's own dependencies of every kind, then the normal and build
/// dependencies of those, which are what building the crate and its tests builds. The shortest chain, by name order
/// among equals, is the problem's path.
pub fn reaches_up(edges: &[Edge]) -> Vec<Problem> {
    let mut layers: BTreeMap<&str, Layer> = BTreeMap::new();
    for e in edges {
        layers.insert(&e.from, e.from_layer);
        layers.insert(&e.to, e.to_layer);
    }
    let mut out = Vec::new();
    for (&start, &from_layer) in &layers {
        let direct: BTreeSet<&str> = edges
            .iter()
            .filter(|e| e.from == start)
            .map(|e| e.to.as_str())
            .collect();
        let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        for &to in &direct {
            if to != start {
                parent.insert(to, start);
                queue.push_back(to);
            }
        }
        while let Some(node) = queue.pop_front() {
            for e in edges
                .iter()
                .filter(|e| e.from == node && e.kind != Kind::Dev)
            {
                if e.to == start || parent.contains_key(e.to.as_str()) {
                    continue;
                }
                parent.insert(&e.to, node);
                queue.push_back(&e.to);
            }
        }
        for &reached in parent.keys() {
            let to_layer = layers[reached];
            if direct.contains(reached) || to_layer <= from_layer || to_layer == Layer::Testing {
                continue;
            }
            out.push(Problem::ReachesUp {
                path: path_to(&parent, start, reached),
                from_layer,
                to_layer,
            });
        }
    }
    out
}

/// The shortest chain in `deps` (each crate's dependencies by name) from `start` to each crate that one of `targets`
/// names, as crate names from `start`.
pub fn crate_paths(
    deps: &BTreeMap<String, BTreeSet<String>>,
    start: &str,
    targets: &[&str],
) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    let mut queue = VecDeque::from([start]);
    while let Some(node) = queue.pop_front() {
        for to in deps.get(node).into_iter().flatten() {
            if to == start || parent.contains_key(to.as_str()) {
                continue;
            }
            parent.insert(to, node);
            if targets.iter().any(|t| matches(t, to)) {
                out.push(path_to(&parent, start, to));
            } else {
                queue.push_back(to);
            }
        }
    }
    out
}

/// A DOT graph of `edges`, one cluster per layer, preceded by comment lines with the crate count per layer and every
/// edge `rules` forbid with its kind.
pub fn dot(title: &str, rules: Rules, layers: &BTreeMap<String, Layer>, edges: &[Edge]) -> String {
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

    let forbidden: Vec<&Edge> = edges.iter().filter(|e| !allows_edge(rules, e)).collect();
    if forbidden.is_empty() {
        let _ = writeln!(s, "// forbidden edges: none");
    }
    for e in &forbidden {
        let _ = writeln!(
            s,
            "// forbidden {} edge {} ({}) -> {} ({})",
            e.kind, e.from, e.from_layer, e.to, e.to_layer
        );
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
        if !allows_edge(rules, e) {
            attrs.push("color=red".to_owned());
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
