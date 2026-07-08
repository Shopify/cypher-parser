//! Tests that a single-variable `WHERE` predicate is pushed into candidate generation even when it
//! is written on a *later* MATCH clause than the one that first binds its variable.
//!
//! Without cross-clause pushdown, `MATCH (t)-[:R]->(c) MATCH (c)-[:S]->(x) WHERE t.name = '...'`
//! materializes the entire `(t)-[:R]->(c)` relation before the `t` filter runs. A spy provider
//! counts how many nodes the first clause expands from, so we can assert that only the matching
//! `t` is expanded, not every candidate.

use std::cell::RefCell;
use std::collections::HashMap;

use cypher_parser::{CypherValue, GraphProvider, execute, parse};

/// A tiny graph: several `Class` nodes, each with one child `Class` via `HAS_CHILD`, and each child
/// owning one `Thing` via `HAS_THING`. Only the "Target" class's subtree should survive the query.
struct SpyGraph {
    labels: Vec<Vec<String>>,
    names: Vec<String>,
    forward: HashMap<(usize, String), Vec<usize>>,
    /// Nodes we were asked to expand along `HAS_CHILD`, in order — the signal that the `t` scan
    /// was (or wasn't) pruned before expansion.
    has_child_expansions: RefCell<Vec<usize>>,
}

impl SpyGraph {
    fn new() -> Self {
        // Classes 0..=3 ("Target", "Other0", "Other1", "Other2"); each has a child class 4..=7;
        // each child owns a Thing 8..=11.
        let mut labels = Vec::new();
        let mut names = Vec::new();
        let mut forward: HashMap<(usize, String), Vec<usize>> = HashMap::new();

        for i in 0..4 {
            labels.push(vec!["Class".to_string()]);
            names.push(if i == 0 {
                "Target".to_string()
            } else {
                format!("Other{}", i - 1)
            });
        }
        for i in 0..4 {
            let child = 4 + i;
            let thing = 8 + i;
            labels.push(vec!["Class".to_string()]); // child class
            names.push(format!("child{i}"));
            forward.insert((i, "HAS_CHILD".to_string()), vec![child]);
            forward.insert((child, "HAS_THING".to_string()), vec![thing]);
        }
        for i in 0..4 {
            labels.push(vec!["Thing".to_string()]);
            names.push(format!("thing{i}"));
        }

        SpyGraph {
            labels,
            names,
            forward,
            has_child_expansions: RefCell::new(Vec::new()),
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
        vec!["HAS_CHILD".to_string(), "HAS_THING".to_string()]
    }

    fn expand(&self, node: usize, rel_type: &str) -> Vec<usize> {
        if rel_type == "HAS_CHILD" {
            self.has_child_expansions.borrow_mut().push(node);
        }
        self.forward
            .get(&(node, rel_type.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn rel_sources(&self, _rel_type: &str) -> Vec<usize> {
        (0..self.labels.len()).collect()
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

const QUERY: &str = "MATCH (t:Class)-[:HAS_CHILD]->(c:Class) \
     MATCH (c)-[:HAS_THING]->(x:Thing) \
     WHERE t.name = 'Target' \
     RETURN x.name AS name";

#[test]
fn later_clause_predicate_prunes_earlier_scan() {
    let graph = SpyGraph::new();
    let result = execute(&graph, &parse(QUERY).unwrap()).unwrap();

    // Correctness: only the Target subtree's Thing survives.
    assert_eq!(result.columns, vec!["name".to_string()]);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0], CypherValue::Str("thing0".to_string()));

    // Optimization: `WHERE t.name = 'Target'` is pushed into the `t` scan, so the first clause
    // expands `HAS_CHILD` from the single matching class only — not from every `Class` node.
    let expanded = graph.has_child_expansions.borrow();
    assert_eq!(
        *expanded,
        vec![0],
        "expected HAS_CHILD to be expanded only from the matching 'Target' class (node 0), \
         but it was expanded from {expanded:?} — the later-clause predicate was not pushed down"
    );
}
