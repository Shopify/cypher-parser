//! Executor tests against a small in-memory `GraphProvider`, demonstrating that the executor is
//! generic over any backend (not tied to any particular graph implementation).

use std::collections::HashMap;

use cypher_parser::{CypherValue, GraphProvider, OutputFormat, execute, parse, run_query};

struct Node {
    labels: Vec<String>,
    props: HashMap<String, String>,
}

struct Edge {
    from: usize,
    rel: String,
    to: usize,
}

#[derive(Default)]
struct TestGraph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

impl TestGraph {
    fn add_node(&mut self, labels: &[&str], props: &[(&str, &str)]) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node {
            labels: labels.iter().map(|s| (*s).to_string()).collect(),
            props: props
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
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

impl GraphProvider for TestGraph {
    type NodeId = usize;

    fn scan(&self, labels: &[String]) -> Vec<usize> {
        (0..self.nodes.len())
            .filter(|id| {
                labels.is_empty() || labels.iter().any(|label| self.matches_label(*id, label))
            })
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
            .map_or(CypherValue::Null, |value| CypherValue::Str(value.clone()))
    }

    fn node_id(&self, node: usize) -> String {
        // A stable, opaque identity. Here we use the node's index; a real provider would encode
        // whatever it needs to round-trip the node (e.g. a type tag plus a primary key).
        format!("node:{node}")
    }

    fn label(&self, node: usize) -> String {
        self.nodes[node].labels.first().cloned().unwrap_or_default()
    }

    fn name(&self, node: usize) -> String {
        self.nodes[node]
            .props
            .get("name")
            .cloned()
            .unwrap_or_default()
    }
}

/// Builds: classes Animal, Dog, Cat; module Walkable.
/// Dog -INHERITS-> Animal, Cat -INHERITS-> Animal, Dog -INCLUDES-> Walkable.
fn fixture() -> TestGraph {
    let mut graph = TestGraph::default();
    let animal = graph.add_node(&["Class"], &[("name", "Animal")]);
    let dog = graph.add_node(&["Class"], &[("name", "Dog")]);
    let cat = graph.add_node(&["Class"], &[("name", "Cat")]);
    let walkable = graph.add_node(&["Module"], &[("name", "Walkable")]);
    graph.add_edge(dog, "INHERITS", animal);
    graph.add_edge(cat, "INHERITS", animal);
    graph.add_edge(dog, "INCLUDES", walkable);
    graph
}

fn column(graph: &TestGraph, query: &str, col: usize) -> Vec<String> {
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

#[test]
fn scan_by_label_and_property() {
    let graph = fixture();
    assert_eq!(
        column(&graph, "MATCH (c:Class {name: 'Dog'}) RETURN c.name", 0),
        vec!["Dog".to_string()]
    );
}

#[test]
fn outgoing_relationship() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class)-[:INHERITS]->(p:Class) WHERE c.name = 'Dog' RETURN p.name",
            0
        ),
        vec!["Animal".to_string()]
    );
}

#[test]
fn incoming_relationship() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (p:Class)<-[:INHERITS]-(c:Class) WHERE p.name = 'Animal' RETURN c.name",
            0
        ),
        vec!["Cat".to_string(), "Dog".to_string()]
    );
}

#[test]
fn label_disjunction() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (n:Class|Module) WHERE n.name = 'Animal' OR n.name = 'Walkable' RETURN n.name",
            0
        ),
        vec!["Animal".to_string(), "Walkable".to_string()]
    );
}

#[test]
fn variable_length() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class)-[:INHERITS*1..]->(a) WHERE c.name = 'Dog' RETURN a.name",
            0
        ),
        vec!["Animal".to_string()]
    );
}

#[test]
fn includes_mixin() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class)-[:INCLUDES]->(m) WHERE c.name = 'Dog' RETURN m.name",
            0
        ),
        vec!["Walkable".to_string()]
    );
}

#[test]
fn aggregation_counts() {
    let graph = fixture();
    let parsed =
        parse("MATCH (c:Class)-[:INHERITS]->(p:Class) WHERE p.name = 'Animal' RETURN p.name, count(c) AS subs")
            .unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0], CypherValue::Str("Animal".into()));
    assert_eq!(result.rows[0][1], CypherValue::Int(2));
}

#[test]
fn distinct_order_limit() {
    let graph = fixture();
    let parsed = parse(
        "MATCH (c:Class)-[:INHERITS]->(p:Class) RETURN DISTINCT p.name ORDER BY p.name LIMIT 1",
    )
    .unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0], CypherValue::Str("Animal".into()));
}

#[test]
fn unknown_relationship_type_errors() {
    let graph = fixture();
    let parsed = parse("MATCH (a)-[:BOGUS]->(b) RETURN a").unwrap();
    assert!(execute(&graph, &parsed).is_err());
}

#[test]
fn in_operator() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE c.name IN ['Dog', 'Cat'] RETURN c.name",
            0
        ),
        vec!["Cat".to_string(), "Dog".to_string()]
    );
}

#[test]
fn is_null_filters() {
    let graph = fixture();
    // No Class node has a `line` property, so IS NULL matches all three classes.
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE c.line IS NULL RETURN c.name",
            0
        ),
        vec!["Animal".to_string(), "Cat".to_string(), "Dog".to_string()]
    );
    // Every class has a name, so IS NULL on name matches nothing and IS NOT NULL matches all.
    assert!(
        column(
            &graph,
            "MATCH (c:Class) WHERE c.name IS NULL RETURN c.name",
            0
        )
        .is_empty()
    );
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE c.name IS NOT NULL RETURN c.name",
            0
        )
        .len(),
        3
    );
}

#[test]
fn return_star_projects_bound_variables() {
    let graph = fixture();
    let parsed =
        parse("MATCH (c:Class)-[:INHERITS]->(p:Class) WHERE c.name = 'Dog' RETURN *").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.columns, vec!["c".to_string(), "p".to_string()]);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0].to_display_string(), "Dog");
    assert_eq!(result.rows[0][1].to_display_string(), "Animal");
}

#[test]
fn return_star_without_variables_errors() {
    let graph = fixture();
    let parsed = parse("MATCH () RETURN *").unwrap();
    assert!(execute(&graph, &parsed).is_err());
}

#[test]
fn scalar_functions() {
    let graph = fixture();

    // toLower used in a WHERE comparison.
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE toLower(c.name) = 'dog' RETURN c.name",
            0
        ),
        vec!["Dog".to_string()]
    );

    // size of a string property.
    let parsed = parse("MATCH (c:Class {name: 'Dog'}) RETURN size(c.name)").unwrap();
    assert_eq!(
        execute(&graph, &parsed).unwrap().rows[0][0],
        CypherValue::Int(3)
    );

    // coalesce returns the first non-null argument.
    let parsed = parse("MATCH (c:Class {name: 'Dog'}) RETURN coalesce(c.nick, c.name)").unwrap();
    assert_eq!(
        execute(&graph, &parsed).unwrap().rows[0][0],
        CypherValue::Str("Dog".into())
    );

    // labels returns the node's label wrapped in a list.
    let parsed = parse("MATCH (c:Class {name: 'Dog'}) RETURN labels(c)").unwrap();
    assert_eq!(
        execute(&graph, &parsed).unwrap().rows[0][0],
        CypherValue::List(vec![CypherValue::Str("Class".into())])
    );
}

#[test]
fn unknown_function_errors() {
    let graph = fixture();
    let parsed = parse("MATCH (c:Class) RETURN bogus(c.name)").unwrap();
    assert!(execute(&graph, &parsed).is_err());
}

#[test]
fn returned_node_carries_id() {
    let graph = fixture();
    // Dog is the second node added (index 1).
    let parsed = parse("MATCH (c:Class {name: 'Dog'}) RETURN c").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    match &result.rows[0][0] {
        CypherValue::Node { id, label, name } => {
            assert_eq!(id, "node:1");
            assert_eq!(label, "Class");
            assert_eq!(name, "Dog");
        }
        other => panic!("expected a node value, got {other:?}"),
    }
}

#[test]
fn distinct_dedupes_nodes_by_identity() {
    let graph = fixture();
    // Dog and Cat both INHERIT Animal, so the same Animal node is produced twice; DISTINCT
    // collapses it to one row because the identity (id) is equal.
    let parsed = parse("MATCH (c:Class)-[:INHERITS]->(p:Class) RETURN DISTINCT p").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0].to_display_string(), "Animal");
}

#[test]
fn distinct_keeps_distinct_nodes_sharing_a_name() {
    // Two separate nodes share the name "Dup" but have distinct ids, so DISTINCT keeps both —
    // identity, not name, drives deduplication.
    let mut graph = TestGraph::default();
    graph.add_node(&["Class"], &[("name", "Dup")]);
    graph.add_node(&["Class"], &[("name", "Dup")]);
    let parsed = parse("MATCH (c:Class) RETURN DISTINCT c").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn with_post_aggregation_filter() {
    let graph = fixture();
    // Animal is inherited by both Dog and Cat (subs = 2); no other class has > 1 subclass.
    let parsed = parse(
        "MATCH (c:Class)-[:INHERITS]->(p:Class) WITH p, count(c) AS subs WHERE subs > 1 RETURN p.name",
    )
    .unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0], CypherValue::Str("Animal".into()));
}

#[test]
fn with_chains_node_bindings() {
    let graph = fixture();
    // `WITH c` keeps c bound as a node, so the next MATCH can expand from it.
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class {name: 'Dog'}) WITH c MATCH (c)-[:INHERITS]->(p) RETURN p.name",
            0
        ),
        vec!["Animal".to_string()]
    );
}

#[test]
fn with_distinct_dedupes_nodes() {
    let graph = fixture();
    // Dog and Cat both inherit Animal; DISTINCT collapses the duplicate Animal binding.
    let parsed =
        parse("MATCH (c:Class)-[:INHERITS]->(p:Class) WITH DISTINCT p RETURN p.name").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0], CypherValue::Str("Animal".into()));
}

#[test]
fn with_order_skip_limit_in_pipeline() {
    let graph = fixture();
    // Classes sorted by name: Animal, Cat, Dog. SKIP 1, LIMIT 1 -> Cat.
    let parsed =
        parse("MATCH (c:Class) WITH c.name AS n ORDER BY n SKIP 1 LIMIT 1 RETURN n").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0][0], CypherValue::Str("Cat".into()));
}

#[test]
fn optional_match_left_join() {
    let graph = fixture();
    // Every class is kept; Animal has no outgoing INHERITS, so its p is null (empty display).
    let parsed =
        parse("MATCH (c:Class) OPTIONAL MATCH (c)-[:INHERITS]->(p:Class) RETURN c.name, p.name")
            .unwrap();
    let result = execute(&graph, &parsed).unwrap();
    let mut rows: Vec<(String, String)> = result
        .rows
        .iter()
        .map(|r| (r[0].to_display_string(), r[1].to_display_string()))
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            ("Animal".to_string(), String::new()),
            ("Cat".to_string(), "Animal".to_string()),
            ("Dog".to_string(), "Animal".to_string()),
        ]
    );
}

#[test]
fn optional_match_is_null_anti_join() {
    let graph = fixture();
    // OPTIONAL MATCH + IS NULL: classes with no outgoing INHERITS -> Animal.
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) OPTIONAL MATCH (c)-[:INHERITS]->(p) WITH c, p WHERE p IS NULL RETURN c.name",
            0
        ),
        vec!["Animal".to_string()]
    );
}

#[test]
fn not_exists_anti_join() {
    let graph = fixture();
    // First-class NOT EXISTS: classes with no outgoing INHERITS edge -> Animal.
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE NOT EXISTS { (c)-[:INHERITS]->() } RETURN c.name",
            0
        ),
        vec!["Animal".to_string()]
    );
}

#[test]
fn exists_predicate_positive() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE EXISTS { (c)-[:INHERITS]->(:Class) } RETURN c.name",
            0
        ),
        vec!["Cat".to_string(), "Dog".to_string()]
    );
}

#[test]
fn exists_predicate_with_inner_where() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) WHERE EXISTS { (c)-[:INHERITS]->(p) WHERE p.name = 'Animal' } RETURN c.name",
            0
        ),
        vec!["Cat".to_string(), "Dog".to_string()]
    );
}

#[test]
fn unwind_expands_list() {
    let graph = fixture();
    let parsed = parse("UNWIND [1, 2, 3] AS x RETURN x").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    let got: Vec<i64> = result.rows.iter().map(|r| r[0].as_int().unwrap()).collect();
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn unwind_after_match_is_cross_product() {
    let graph = fixture();
    // 3 classes x 2 elements = 6 rows.
    let parsed = parse("MATCH (c:Class) UNWIND [1, 2] AS n RETURN c.name, n").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(result.rows.len(), 6);
}

#[test]
fn case_expression() {
    let graph = fixture();
    let parsed =
        parse("MATCH (c:Class) RETURN c.name, CASE c.name WHEN 'Dog' THEN 'woof' ELSE '?' END")
            .unwrap();
    let result = execute(&graph, &parsed).unwrap();

    let dog = result
        .rows
        .iter()
        .find(|r| r[0] == CypherValue::Str("Dog".into()))
        .unwrap();
    assert_eq!(dog[1], CypherValue::Str("woof".into()));

    let animal = result
        .rows
        .iter()
        .find(|r| r[0] == CypherValue::Str("Animal".into()))
        .unwrap();
    assert_eq!(animal[1], CypherValue::Str("?".into()));
}

#[test]
fn comments_are_ignored() {
    let graph = fixture();
    assert_eq!(
        column(
            &graph,
            "MATCH (c:Class) // only dogs\n WHERE c.name = 'Dog' /* comment */ RETURN c.name",
            0
        ),
        vec!["Dog".to_string()]
    );
}

#[test]
fn map_projection_builds_map() {
    let graph = fixture();
    let parsed = parse("MATCH (c:Class {name: 'Dog'}) RETURN c { .name, kind: 'class' }").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(
        result.rows[0][0],
        CypherValue::Map(vec![
            ("name".to_string(), CypherValue::Str("Dog".into())),
            ("kind".to_string(), CypherValue::Str("class".into())),
        ])
    );
}

#[test]
fn negative_literal_comparison() {
    let graph = fixture();
    let parsed = parse("UNWIND [-5, -1, 3] AS x WITH x WHERE x > -2 RETURN x").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    let got: Vec<i64> = result.rows.iter().map(|r| r[0].as_int().unwrap()).collect();
    assert_eq!(got, vec![-1, 3]);
}

#[test]
fn order_by_cross_type_orderability() {
    let graph = fixture();
    // openCypher orderability ascending: string < boolean < number < null.
    let parsed = parse("UNWIND ['a', true, 1, null] AS x RETURN x ORDER BY x").unwrap();
    let result = execute(&graph, &parsed).unwrap();
    assert_eq!(
        result.rows,
        vec![
            vec![CypherValue::Str("a".into())],
            vec![CypherValue::Bool(true)],
            vec![CypherValue::Int(1)],
            vec![CypherValue::Null],
        ]
    );
}

#[test]
fn run_query_json() {
    let graph = fixture();
    let output = run_query(
        &graph,
        "MATCH (c:Class {name: 'Dog'}) RETURN c.name",
        OutputFormat::Json,
    )
    .unwrap();
    assert_eq!(output, "[{\"c.name\":\"Dog\"}]");
}

#[test]
fn node_json_does_not_leak_id() {
    let graph = fixture();
    let output = run_query(
        &graph,
        "MATCH (c:Class {name: 'Dog'}) RETURN c",
        OutputFormat::Json,
    )
    .unwrap();
    // The opaque node id is used for identity only and must never appear in the output.
    assert_eq!(output, "[{\"c\":{\"label\":\"Class\",\"name\":\"Dog\"}}]");
}
