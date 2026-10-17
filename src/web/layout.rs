//! Pure graph layout for the dashboard Graph view (SPEC-graph-view, FR-700).
//!
//! Assigns each node a `layer` (longest dependency depth from the roots) and
//! an `order` (stable position within the layer, by plan order). Deterministic
//! and cycle-safe: unknown endpoints and self-loops are ignored, and cycle
//! nodes are placed after their already-placed predecessors in input order.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

/// Position of a node in the layered graph drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NodeLayout {
    /// Longest-path depth from the roots (roots are layer 0).
    pub layer: u32,
    /// Stable position within the layer, by input node order.
    pub order: u32,
}

/// Lay out a graph as layers.
///
/// Edges are `(from, to)` = `dependency -> dependent`. Edges whose endpoints
/// are unknown and self-loops are ignored. The result is deterministic for any
/// input, including cycles, and terminates.
pub fn layout_graph(
    node_ids: &[&str],
    edges: &[(&str, &str)],
) -> std::collections::BTreeMap<String, NodeLayout> {
    let mut index: BTreeMap<&str, usize> = BTreeMap::new();
    for (position, id) in node_ids.iter().enumerate() {
        index.entry(*id).or_insert(position);
    }

    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); node_ids.len()];
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); node_ids.len()];
    let mut indegree: Vec<usize> = vec![0; node_ids.len()];
    let mut seen: BTreeSet<(usize, usize)> = BTreeSet::new();

    for (from, to) in edges {
        if from == to {
            continue;
        }
        let (Some(&source), Some(&target)) = (index.get(*from), index.get(*to)) else {
            continue;
        };
        if !seen.insert((source, target)) {
            continue;
        }
        adjacency[source].push(target);
        predecessors[target].push(source);
        indegree[target] += 1;
    }

    let mut layer: Vec<Option<u32>> = vec![None; node_ids.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (position, &degree) in indegree.iter().enumerate() {
        if degree == 0 {
            layer[position] = Some(0);
            queue.push_back(position);
        }
    }

    // Kahn's algorithm: a node is queued once all of its predecessors have been
    // placed, so `layer` reaches the longest-path depth.
    while let Some(node) = queue.pop_front() {
        let candidate = layer[node].unwrap_or(0) + 1;
        for &dependent in &adjacency[node] {
            if layer[dependent].is_none_or(|current| current < candidate) {
                layer[dependent] = Some(candidate);
            }
            indegree[dependent] -= 1;
            if indegree[dependent] == 0 {
                queue.push_back(dependent);
            }
        }
    }

    // Any node still unplaced is in a cycle. Place it after the maximum layer of
    // its already-placed predecessors, in input order (deterministic).
    for position in 0..node_ids.len() {
        if layer[position].is_some() {
            continue;
        }
        let max_pred = predecessors[position]
            .iter()
            .filter_map(|&pred| layer[pred])
            .max();
        layer[position] = Some(max_pred.map_or(0, |depth| depth + 1));
    }

    let mut by_layer: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (position, depth) in layer.iter().enumerate() {
        if let Some(depth) = depth {
            by_layer.entry(*depth).or_default().push(position);
        }
    }

    let mut result: BTreeMap<String, NodeLayout> = BTreeMap::new();
    for (depth, mut members) in by_layer {
        members.sort_unstable();
        for (order, position) in members.into_iter().enumerate() {
            result.insert(
                node_ids[position].to_string(),
                NodeLayout {
                    layer: depth,
                    order: order as u32,
                },
            );
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn at(layer: u32, order: u32) -> NodeLayout {
        NodeLayout { layer, order }
    }

    // TC-700 / FR-700: diamond + isolated node + edge to a missing node.
    #[test]
    fn layout_assigns_layers_and_stable_order() {
        let ids = ["a", "b", "c", "d", "e"];
        let edges = [
            ("a", "b"),
            ("a", "c"),
            ("b", "d"),
            ("c", "d"),
            ("e", "missing"),
        ];

        let layout = layout_graph(&ids, &edges);

        assert_eq!(layout.len(), 5);
        assert_eq!(layout["a"], at(0, 0));
        assert_eq!(layout["b"], at(1, 0));
        assert_eq!(layout["c"], at(1, 1));
        assert_eq!(layout["d"], at(2, 0));
        assert_eq!(layout["e"], at(0, 1));
        assert!(!layout.contains_key("missing"));

        // Stable across runs.
        assert_eq!(layout, layout_graph(&ids, &edges));
    }

    // TC-701 / FR-700: cycles, self-loops, and the empty plan.
    #[test]
    fn layout_is_cycle_safe_and_handles_empty() {
        let ids = ["r", "x", "y"];
        let edges = [("r", "x"), ("x", "y"), ("y", "x"), ("x", "x")];

        let layout = layout_graph(&ids, &edges);

        assert_eq!(layout.len(), 3);
        assert_eq!(layout["r"], at(0, 0));
        assert_eq!(layout["x"], at(1, 0));
        assert_eq!(layout["y"], at(2, 0));
        for id in ids {
            assert!(layout.contains_key(id), "missing layer for {id}");
        }
        // Deterministic for cyclic input.
        assert_eq!(layout, layout_graph(&ids, &edges));

        // Empty plan -> empty layout.
        assert!(layout_graph(&[], &[]).is_empty());
    }
}
