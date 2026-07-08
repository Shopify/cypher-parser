//! Tests for the targeted incoming-expansion hook `GraphProvider::expand_in`.
//!
//! A spy provider counts calls to `rel_sources` (the whole-graph reverse-adjacency source) so we
//! can assert that an incoming step uses the O(degree) `expand_in` fast path when the provider
//! implements it, and falls back to `rel_sources` when it does not.

use std::cell::Cell;
use std::collections::HashMap;

use cypher_parser::{CypherValue, GraphProvider, execute, parse};

struct SpyGraph {
    labels: Vec<Vec<String>>,
    names: Vec<String>,
    /// Forward edges: (from, rel) -> targets.
    forward: HashMap<(usize, String), Vec<usize>>,
    /// Reverse edges: (to, rel) -> sources, used to answer `expand_in` directly.
    reverse: HashMap<(usize, String), Vec<usize>>,
    /// Whether `expand_in` answers directly (true) or returns `None` to force the fallback.
    use_expand_in: bool,
    rel_sources_calls: Cell<usize>,
}

impl SpyGraph {
    /// Class "Shop" (node 0) with two definitions (nodes 1, 2), each `-[:DECLARES]-> Shop`.
    fn new(use_expand_in: bool) -> Self {
        let mut forward: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        let mut reverse: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        for def in [1usize, 2] {
            forward.insert((def, "DECLARES".to_string()), vec![0]);
        }
        reverse.insert((0, "DECLARES".to_string()), vec![1, 2]);

        SpyGraph {
            labels: vec![
                vec!["Class".to_string()],
                vec!["Definition".to_string()],
                vec!["Definition".to_string()],
            ],
            names: vec!["Shop".to_string(), "def1".to_string(), "def2".to_string()],
            forward,
            reverse,
            use_expand_in,
            rel_sources_calls: Cell::new(0),
        }
    }
}

impl GraphProvider for SpyGraph {
    type NodeId = usize;

    fn scan(&self, labels: &[String]) -> Vec<usize> {
        (0..self.labels.len())
            .filter(|id| labels.is_empty() || labels.iter().any(|l| self.matches_label(*id, l)))
            .collect()
    }

    fn matches_label(&self, node: usize, label: &str) -> bool {
        self.labels[node].iter().any(|l| l == label)
    }

    fn relationship_types(&self) -> Vec<String> {
        vec!["DECLARES".to_string()]
    }

    fn expand(&self, node: usize, rel_type: &str) -> Vec<usize> {
        self.forward
            .get(&(node, rel_type.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn rel_sources(&self, _rel_type: &str) -> Vec<usize> {
        self.rel_sources_calls.set(self.rel_sources_calls.get() + 1);
        (0..self.labels.len()).collect()
    }

    fn expand_in(&self, node: usize, rel_type: &str) -> Option<Vec<usize>> {
        if self.use_expand_in {
            Some(
                self.reverse
                    .get(&(node, rel_type.to_string()))
                    .cloned()
                    .unwrap_or_default(),
            )
        } else {
            None
        }
    }

    fn property(&self, node: usize, prop: &str) -> CypherValue {
        if prop == "name" {
            CypherValue::Str(self.names[node].clone())
        } else {
            CypherValue::Null
        }
    }

    fn node_id(&self, node: usize) -> String {
        format!("n{node}")
    }

    fn label(&self, node: usize) -> String {
        self.labels[node].first().cloned().unwrap_or_default()
    }

    fn name(&self, node: usize) -> String {
        self.names[node].clone()
    }
}

const QUERY: &str = "MATCH (c:Class)<-[:DECLARES]-(d) WHERE c.name = 'Shop' RETURN count(d)";

#[test]
fn expand_in_avoids_reverse_build() {
    let graph = SpyGraph::new(true);
    let result = execute(&graph, &parse(QUERY).unwrap()).unwrap();

    assert_eq!(result.rows[0][0], CypherValue::Int(2));
    assert_eq!(
        graph.rel_sources_calls.get(),
        0,
        "expand_in should serve the incoming step without a whole-graph reverse build"
    );
}

#[test]
fn incoming_falls_back_when_expand_in_returns_none() {
    let graph = SpyGraph::new(false);
    let result = execute(&graph, &parse(QUERY).unwrap()).unwrap();

    assert_eq!(result.rows[0][0], CypherValue::Int(2));
    assert!(
        graph.rel_sources_calls.get() > 0,
        "without expand_in the executor should fall back to the rel_sources reverse build"
    );
}
