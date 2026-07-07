//! Scale / parity tests on a larger synthetic graph.
//!
//! These exercise the performance-oriented executor changes (bound-endpoint anchoring, HashSet
//! DISTINCT, hoisted `IN`, WHERE pushdown) for *correctness*: the same logical query written in
//! different shapes must return identical results. They also serve as a coarse regression guard —
//! before anchoring, the forward-written existential form was O(rows x nodes) and would not finish
//! in reasonable time on this graph; it now runs through the incoming cache and completes quickly.

use std::collections::HashMap;

use cypher_parser::{CypherValue, GraphProvider, execute, parse};

struct Node {
    labels: Vec<String>,
    props: HashMap<String, CypherValue>,
}

struct Edge {
    from: usize,
    rel: String,
}

#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    // Adjacency for fast expand: (from, rel) -> targets.
    out: HashMap<(usize, String), Vec<usize>>,
}

impl Graph {
    fn add_node(&mut self, label: &str, name: &str) -> usize {
        let id = self.nodes.len();
        let mut props = HashMap::new();
        props.insert("name".to_string(), CypherValue::Str(name.to_string()));
        self.nodes.push(Node {
            labels: vec![label.to_string()],
            props,
        });
        id
    }

    fn add_edge(&mut self, from: usize, rel: &str, to: usize) {
        self.edges.push(Edge {
            from,
            rel: rel.to_string(),
        });
        self.out
            .entry((from, rel.to_string()))
            .or_default()
            .push(to);
    }
}

impl GraphProvider for Graph {
    type NodeId = usize;

    fn scan(&self, labels: &[String]) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|id| labels.is_empty() || labels.iter().any(|l| self.matches_label(*id, l)))
            .collect()
    }

    fn matches_label(&self, node: usize, label: &str) -> bool {
        self.nodes[node].labels.iter().any(|l| l == label)
    }

    fn relationship_types(&self) -> Vec<String> {
        let mut types: Vec<String> = Vec::new();
        for edge in &self.edges {
            if !types.contains(&edge.rel) {
                types.push(edge.rel.clone());
            }
        }
        types
    }

    fn expand(&self, node: usize, rel_type: &str) -> Vec<usize> {
        self.out
            .get(&(node, rel_type.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn rel_sources(&self, rel_type: &str) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|e| e.rel == rel_type)
            .map(|e| e.from)
            .collect()
    }

    fn property(&self, node: usize, prop: &str) -> CypherValue {
        self.nodes[node]
            .props
            .get(prop)
            .cloned()
            .unwrap_or(CypherValue::Null)
    }

    fn node_id(&self, node: usize) -> String {
        format!("n{node}")
    }

    fn label(&self, node: usize) -> String {
        self.nodes[node].labels.first().cloned().unwrap_or_default()
    }

    fn name(&self, node: usize) -> String {
        match self.nodes[node].props.get("name") {
            Some(CypherValue::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
}

const DECLS: usize = 5_000;
const DOCS: usize = 500;
const REFERENCED: usize = 2_000;

/// Builds a graph of `DECLS` `:Decl` nodes and `DOCS` `:Doc` nodes, where every `Decl` in
/// `[0, REFERENCED)` has at least one incoming `REFERENCES` edge from a `Doc` (with a couple of
/// very high-fan-in targets), and `Decl`s in `[REFERENCED, DECLS)` are unreferenced ("dead").
fn corpus() -> Graph {
    let mut g = Graph::default();
    let decls: Vec<usize> = (0..DECLS)
        .map(|i| g.add_node("Decl", &format!("d{i}")))
        .collect();
    let docs: Vec<usize> = (0..DOCS)
        .map(|i| g.add_node("Doc", &format!("doc{i}")))
        .collect();

    // Cover every referenced decl with at least one incoming edge.
    for i in 0..REFERENCED {
        g.add_edge(docs[i % DOCS], "REFERENCES", decls[i]);
    }
    // High fan-in: every doc references decls 0 and 1.
    for &doc in &docs {
        g.add_edge(doc, "REFERENCES", decls[0]);
        g.add_edge(doc, "REFERENCES", decls[1]);
    }

    g
}

fn names(graph: &Graph, query: &str, col: usize) -> Vec<String> {
    let parsed = parse(query).unwrap();
    let result = execute(graph, &parsed).unwrap();
    let mut values: Vec<String> = result
        .rows
        .iter()
        .map(|row| row[col].to_display_string())
        .collect();
    values.sort();
    values
}

/// #1 anchoring: the forward-written existential (`(x)-[:R]->(d)`, unbound source) must return the
/// same result as the incoming-written one (`(d)<-[:R]-(x)`, anchored on bound `d`), and finish.
#[test]
fn forward_and_incoming_not_exists_agree() {
    let graph = corpus();

    let forward = names(
        &graph,
        "MATCH (d:Decl) WHERE NOT EXISTS { (x)-[:REFERENCES]->(d) } RETURN d.name",
        0,
    );
    let incoming = names(
        &graph,
        "MATCH (d:Decl) WHERE NOT EXISTS { (d)<-[:REFERENCES]-(x) } RETURN d.name",
        0,
    );

    assert_eq!(forward, incoming);
    assert_eq!(forward.len(), DECLS - REFERENCED);
}

/// #2 DISTINCT + #3 hoisted IN: the `WITH collect(DISTINCT ...) ... WHERE NOT d IN used` shape must
/// return the same dead-declaration set as the existential form.
#[test]
fn not_exists_matches_collect_in() {
    let graph = corpus();

    let not_exists = names(
        &graph,
        "MATCH (d:Decl) WHERE NOT EXISTS { (d)<-[:REFERENCES]-(x) } RETURN d.name",
        0,
    );
    let collect_in = names(
        &graph,
        "MATCH (doc:Doc)-[:REFERENCES]->(u:Decl) \
         WITH collect(DISTINCT u) AS used \
         MATCH (d:Decl) WHERE NOT d IN used RETURN d.name",
        0,
    );

    assert_eq!(collect_in, not_exists);
    assert_eq!(collect_in.len(), DECLS - REFERENCED);
}

/// #2 DISTINCT at scale: distinct referenced declarations equals the referenced count.
#[test]
fn count_distinct_referenced() {
    let graph = corpus();
    let parsed = parse("MATCH (doc:Doc)-[:REFERENCES]->(u:Decl) RETURN count(DISTINCT u)").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows[0][0], CypherValue::Int(REFERENCED as i64));
}

/// #4 pushdown correctness: a single-variable property predicate still returns the exact match
/// (the scan is pruned before expansion, but results are unchanged).
#[test]
fn pushdown_property_predicate() {
    let graph = corpus();
    assert_eq!(
        names(
            &graph,
            "MATCH (d:Decl) WHERE d.name = 'd1234' RETURN d.name",
            0
        ),
        vec!["d1234".to_string()]
    );
}
