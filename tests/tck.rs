//! A small conformance harness that runs hand-ported, read-only scenarios from the openCypher
//! Technology Compatibility Kit (TCK) against this crate's executor.
//!
//! The TCK (<https://github.com/opencypher/openCypher>, `tck/features`) is a set of Cucumber
//! `.feature` files. We do not run them wholesale: most scenarios build their graph with write
//! clauses (`CREATE`), which this read-only crate intentionally does not support, and several use a
//! value/result model (inline node properties, multi-labels, paths) richer than our `CypherValue`.
//! Instead, we port a curated subset:
//!
//! - Scenarios on the TCK's fixed **named graphs** (here, `binary-tree-1`), which need no `CREATE`.
//! - Scenarios that return **scalars**, so the expected-result table maps onto our value model.
//!
//! Each test cites the source feature file and scenario so it can be checked against the upstream
//! TCK. Grow this set as the supported subset grows (e.g. `WITH` unlocks the triadic-selection
//! scenarios that filter with `OPTIONAL MATCH ... WITH`).
//!
//! The ported queries and expected results are derived from the openCypher TCK, which is licensed
//! under the Apache License, Version 2.0. This work was created by the collective efforts of the
//! openCypher community (Cypher is a registered trademark of Neo4j Inc.).

use std::collections::HashMap;

use cypher_parser::{CypherValue, GraphProvider, execute, parse};

// ---- A flexible in-memory provider (supports typed properties) -----------------------------

struct Node {
    labels: Vec<String>,
    props: HashMap<String, CypherValue>,
}

struct Edge {
    from: usize,
    rel: String,
    to: usize,
}

#[derive(Default)]
struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

impl Graph {
    fn add_node(&mut self, labels: &[&str], props: &[(&str, CypherValue)]) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            labels: labels.iter().map(|s| (*s).to_string()).collect(),
            props: props
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        });
        id
    }

    fn add_edge(&mut self, from: usize, rel: &str, to: usize) {
        self.edges.push(Edge {
            from,
            rel: rel.to_string(),
            to,
        });
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
        self.edges
            .iter()
            .filter(|e| e.from == node && e.rel == rel_type)
            .map(|e| e.to)
            .collect()
    }

    fn rel_sources(&self, _rel_type: &str) -> Vec<usize> {
        (0..self.nodes.len()).collect()
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

// ---- Named graph: binary-tree-1 ------------------------------------------------------------
//
// Faithful to tck/graphs/binary-tree-1/binary-tree-1.cypher:
//   (a:A), (b1..b4:X), (c11..c42:X)
//   a-[:KNOWS]->b1, a-[:KNOWS]->b2, a-[:FOLLOWS]->b3, a-[:FOLLOWS]->b4
//   b{n}-[:FRIEND]->c{n}1, b{n}-[:FRIEND]->c{n}2
//   b1-[:FRIEND]->b2, b2-[:FRIEND]->b3, b3-[:FRIEND]->b4, b4-[:FRIEND]->b1
fn binary_tree_1() -> Graph {
    let mut g = Graph::default();
    let name = |s: &str| (("name"), CypherValue::Str(s.to_string()));

    let a = g.add_node(&["A"], &[name("a")]);
    let b1 = g.add_node(&["X"], &[name("b1")]);
    let b2 = g.add_node(&["X"], &[name("b2")]);
    let b3 = g.add_node(&["X"], &[name("b3")]);
    let b4 = g.add_node(&["X"], &[name("b4")]);
    let c11 = g.add_node(&["X"], &[name("c11")]);
    let c12 = g.add_node(&["X"], &[name("c12")]);
    let c21 = g.add_node(&["X"], &[name("c21")]);
    let c22 = g.add_node(&["X"], &[name("c22")]);
    let c31 = g.add_node(&["X"], &[name("c31")]);
    let c32 = g.add_node(&["X"], &[name("c32")]);
    let c41 = g.add_node(&["X"], &[name("c41")]);
    let c42 = g.add_node(&["X"], &[name("c42")]);

    g.add_edge(a, "KNOWS", b1);
    g.add_edge(a, "KNOWS", b2);
    g.add_edge(a, "FOLLOWS", b3);
    g.add_edge(a, "FOLLOWS", b4);

    g.add_edge(b1, "FRIEND", c11);
    g.add_edge(b1, "FRIEND", c12);
    g.add_edge(b2, "FRIEND", c21);
    g.add_edge(b2, "FRIEND", c22);
    g.add_edge(b3, "FRIEND", c31);
    g.add_edge(b3, "FRIEND", c32);
    g.add_edge(b4, "FRIEND", c41);
    g.add_edge(b4, "FRIEND", c42);

    g.add_edge(b1, "FRIEND", b2);
    g.add_edge(b2, "FRIEND", b3);
    g.add_edge(b3, "FRIEND", b4);
    g.add_edge(b4, "FRIEND", b1);

    g
}

// ---- Helpers -------------------------------------------------------------------------------

/// Runs a query and returns the values in one column as a sorted multiset of display strings.
/// Suitable for `Then the result should be, in any order:` over scalar columns.
fn column_sorted(graph: &Graph, query: &str, col: usize) -> Vec<String> {
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

fn sorted(values: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = values.iter().map(|s| (*s).to_string()).collect();
    v.sort();
    v
}

/// Runs a query and returns one column's values in result order (for `Then the result should be,
/// in order:` scenarios). Does not sort.
fn column_in_order(graph: &Graph, query: &str, col: usize) -> Vec<String> {
    let parsed = parse(query).unwrap();
    let result = execute(graph, &parsed).unwrap();
    result
        .rows
        .iter()
        .map(|row| row[col].to_display_string())
        .collect()
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| (*s).to_string()).collect()
}

// ---- Ported scenarios ----------------------------------------------------------------------

/// TCK: tck/features/useCases/triadicSelection/TriadicSelection1.feature
/// Scenario: [1] Handling triadic friend of a friend
#[test]
fn triadic_selection_1_friend_of_a_friend() {
    let graph = binary_tree_1();
    let got = column_sorted(&graph, "MATCH (a:A)-[:KNOWS]->(b)-->(c) RETURN c.name", 0);
    assert_eq!(got, sorted(&["b2", "b3", "c11", "c12", "c21", "c22"]));
}

// ---- Named graph: Match5 background (depth-4 LIKES tree) -----------------------------------
//
// Faithful to the Background of tck/features/clauses/match/Match5.feature:
//   n0:A; n00,n01:B; n000,n001,n010,n011:C; n0000..n0111:D (8 leaves)
//   each node LIKES its two children, forming a complete binary tree of depth 4.
fn match5_tree() -> Graph {
    let mut g = Graph::default();
    let name = |s: &str| ("name", CypherValue::Str(s.to_string()));
    let id = |g: &mut Graph, label: &str, n: &str| g.add_node(&[label], &[name(n)]);

    let n0 = id(&mut g, "A", "n0");
    let n00 = id(&mut g, "B", "n00");
    let n01 = id(&mut g, "B", "n01");
    let n000 = id(&mut g, "C", "n000");
    let n001 = id(&mut g, "C", "n001");
    let n010 = id(&mut g, "C", "n010");
    let n011 = id(&mut g, "C", "n011");
    let n0000 = id(&mut g, "D", "n0000");
    let n0001 = id(&mut g, "D", "n0001");
    let n0010 = id(&mut g, "D", "n0010");
    let n0011 = id(&mut g, "D", "n0011");
    let n0100 = id(&mut g, "D", "n0100");
    let n0101 = id(&mut g, "D", "n0101");
    let n0110 = id(&mut g, "D", "n0110");
    let n0111 = id(&mut g, "D", "n0111");

    for (from, to) in [
        (n0, n00),
        (n0, n01),
        (n00, n000),
        (n00, n001),
        (n01, n010),
        (n01, n011),
        (n000, n0000),
        (n000, n0001),
        (n001, n0010),
        (n001, n0011),
        (n010, n0100),
        (n010, n0101),
        (n011, n0110),
        (n011, n0111),
    ] {
        g.add_edge(from, "LIKES", to);
    }

    g
}

/// TCK: tck/features/clauses/match/Match5.feature
/// Scenarios [1]-[13] — variable-length matching over the depth-4 LIKES tree.
///
/// The TCK writes each query as two clauses, `MATCH (a:A) MATCH (a)-[:LIKES*N]->(c)`. This crate
/// does not yet support multiple `MATCH` clauses, so we use the semantically identical single
/// pattern `MATCH (a:A)-[:LIKES*N]->(c)`. Expected results are taken verbatim from the TCK.
#[test]
fn match5_variable_length() {
    let graph = match5_tree();

    let all_descendants = &[
        "n00", "n01", "n000", "n001", "n010", "n011", "n0000", "n0001", "n0010", "n0011", "n0100",
        "n0101", "n0110", "n0111",
    ];

    // (scenario, var-length spec, expected c.name values)
    let cases: &[(&str, &str, &[&str])] = &[
        ("[1] unbounded", "*", all_descendants),
        ("[2] explicitly unbounded", "*..", all_descendants),
        ("[3] *0", "*0", &["n0"]),
        ("[4] *1", "*1", &["n00", "n01"]),
        ("[5] *2", "*2", &["n000", "n001", "n010", "n011"]),
        (
            "[6] *0..2",
            "*0..2",
            &["n0", "n00", "n01", "n000", "n001", "n010", "n011"],
        ),
        (
            "[7] *1..2",
            "*1..2",
            &["n00", "n01", "n000", "n001", "n010", "n011"],
        ),
        ("[8] *0..0", "*0..0", &["n0"]),
        ("[9] *1..1", "*1..1", &["n00", "n01"]),
        ("[10] *2..2", "*2..2", &["n000", "n001", "n010", "n011"]),
        ("[11] *2..1 (empty)", "*2..1", &[]),
        ("[12] *1..0 (empty)", "*1..0", &[]),
        ("[13] *..0 (empty)", "*..0", &[]),
    ];

    for (scenario, spec, expected) in cases {
        let query = format!("MATCH (a:A)-[:LIKES{spec}]->(c) RETURN c.name");
        let got = column_sorted(&graph, &query, 0);
        assert_eq!(got, sorted(expected), "Match5 scenario {scenario}");
    }
}

/// TCK: tck/features/useCases/triadicSelection/TriadicSelection1.feature
/// Scenario: [2] Handling triadic friend of a friend that is not a friend.
///
/// The TCK writes the anti-join with a relationship variable and `OPTIONAL MATCH (a)-[r:KNOWS]->(c)
/// WITH c WHERE r IS NULL`. This crate does not bind relationship variables, so we express the same
/// "c is not a KNOWS-target of a" semantics with `NOT EXISTS`. Expected results are verbatim.
#[test]
fn triadic_selection_1_not_a_friend() {
    let graph = binary_tree_1();
    let got = column_sorted(
        &graph,
        "MATCH (a:A)-[:KNOWS]->(b)-->(c) WHERE NOT EXISTS { (a)-[:KNOWS]->(c) } RETURN c.name",
        0,
    );
    assert_eq!(got, sorted(&["b3", "c11", "c12", "c21", "c22"]));
}

/// TCK: tck/features/clauses/return-orderby/ReturnOrderBy1.feature
/// Scenarios [1]-[6] — `UNWIND [...] AS x RETURN x ORDER BY x [DESC]` over homogeneous element
/// types (booleans, strings, integers). Now portable because `UNWIND` can start a query.
///
/// Not ported: [7]/[8] (floats — unsupported) and [9]/[10] (lists mixing types and nulls — Cypher's
/// cross-type orderability, with nulls sorting last, differs from this crate's comparator).
#[test]
fn return_order_by_1_homogeneous() {
    let graph = Graph::default();

    // (scenario, query, expected in order)
    let cases: &[(&str, &str, Vec<String>)] = &[
        (
            "[1] booleans asc",
            "UNWIND [true, false] AS bools RETURN bools ORDER BY bools",
            owned(&["false", "true"]),
        ),
        (
            "[2] booleans desc",
            "UNWIND [true, false] AS bools RETURN bools ORDER BY bools DESC",
            owned(&["true", "false"]),
        ),
        (
            "[3] strings asc",
            "UNWIND ['.*', '', ' ', 'one'] AS strings RETURN strings ORDER BY strings",
            owned(&["", " ", ".*", "one"]),
        ),
        (
            "[4] strings desc",
            "UNWIND ['.*', '', ' ', 'one'] AS strings RETURN strings ORDER BY strings DESC",
            owned(&["one", ".*", " ", ""]),
        ),
        (
            "[5] ints asc",
            "UNWIND [1, 3, 2] AS ints RETURN ints ORDER BY ints",
            owned(&["1", "2", "3"]),
        ),
        (
            "[6] ints desc",
            "UNWIND [1, 3, 2] AS ints RETURN ints ORDER BY ints DESC",
            owned(&["3", "2", "1"]),
        ),
    ];

    for (scenario, query, expected) in cases {
        assert_eq!(&column_in_order(&graph, query, 0), expected, "{scenario}");
    }
}

/// TCK: tck/features/clauses/match/Match1.feature
/// Scenario: [1] Match non-existent nodes returns empty
#[test]
fn match1_match_non_existent_nodes_returns_empty() {
    let graph = Graph::default();
    let parsed = parse("MATCH (n) RETURN n").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert!(result.rows.is_empty());
}

/// TCK: tck/features/clauses/match/Match1.feature
/// Scenario: [5] Use multiple MATCH clauses to do a Cartesian product
/// (Ported with the setup graph `CREATE ({num: 1}), ({num: 2}), ({num: 3})`.)
#[test]
fn match1_cartesian_product() {
    let mut graph = Graph::default();
    for n in 1..=3 {
        graph.add_node(&[], &[("num", CypherValue::Int(n))]);
    }

    let parsed = parse("MATCH (n), (m) RETURN n.num AS n, m.num AS m").unwrap();
    let result = execute(&graph, &parsed).unwrap();

    let mut got: Vec<(i64, i64)> = result
        .rows
        .iter()
        .map(|row| (row[0].as_int().unwrap(), row[1].as_int().unwrap()))
        .collect();
    got.sort_unstable();

    let mut expected: Vec<(i64, i64)> =
        (1..=3).flat_map(|n| (1..=3).map(move |m| (n, m))).collect();
    expected.sort_unstable();

    assert_eq!(got, expected);
}
