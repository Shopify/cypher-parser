# cypher-parser

A small, dependency-free lexer, parser, and pluggable executor for a practical subset of the
[Cypher](https://opencypher.org/) graph query language.

It has two layers:

1. **Parsing** turns a query string into an AST (or a positioned error).
2. **Execution** evaluates a parsed query against *any* backend that implements the
   [`GraphProvider`] trait, returning a result set you can render as a table or JSON. The executor
   is generic and reads your graph only through that trait.

It targets read-only, introspection-style queries.

## Parsing

```rust
use cypher_parser::parse;

let query = parse(
    "MATCH (c:Class)-[:INHERITS*1..]->(p:Class {name: 'Base'}) \
     WHERE c.name CONTAINS 'Service' \
     RETURN c.name, count(*) AS total \
     ORDER BY total DESC LIMIT 10",
)?;

for item in &query.result.items {
    println!("{}", item.column_name());
}
# Ok::<(), cypher_parser::CypherError>(())
```

## Executing against your own graph

Implement [`GraphProvider`] for your data structure, then call `run_query` (or `execute` for a
structured `ResultSet`):

```rust,ignore
use cypher_parser::{run_query, GraphProvider, CypherValue, OutputFormat};

impl GraphProvider for MyGraph {
    type NodeId = usize;
    fn scan(&self, labels: &[String]) -> Vec<usize> { /* ... */ }
    fn matches_label(&self, node: usize, label: &str) -> bool { /* ... */ }
    fn relationship_types(&self) -> Vec<String> { /* ... */ }
    fn expand(&self, node: usize, rel_type: &str) -> Vec<usize> { /* ... */ }
    fn rel_sources(&self, rel_type: &str) -> Vec<usize> { /* ... */ }
    fn property(&self, node: usize, prop: &str) -> CypherValue { /* ... */ }
    fn node_id(&self, node: usize) -> String { /* stable, opaque identity */ }
    fn label(&self, node: usize) -> String { /* ... */ }
    fn name(&self, node: usize) -> String { /* ... */ }
}

let output = run_query(&my_graph, "MATCH (n:Class) RETURN n.name", OutputFormat::Json)?;
```

## Supported subset

- **`MATCH`** — node patterns `(v:Label {prop: value})` with label disjunction (`(v:A|B)`); relationship
  patterns `-[:TYPE]->`, `<-[:TYPE]-`, `-[:TYPE]-`, including variable-length `-[:TYPE*min..max]->`.
- **`WHERE`** — `=`, `<>`, `<`, `<=`, `>`, `>=`, `CONTAINS`, `STARTS WITH`, `ENDS WITH`, `IN` (with
  `[...]` list literals), `IS NULL` / `IS NOT NULL`, combined with `AND`, `OR`, `NOT`.
- **Scalar functions** — `toLower`, `toUpper`, `size`, `coalesce`, `labels`.
- **`RETURN`** — `DISTINCT`, `*` (all bound variables), `AS` aliases, and the aggregates `count`,
  `collect`, `min`, `max`, `sum`, `avg`.
- **`WITH`** — chain clauses by projecting into new bindings (same features as `RETURN`, including
  aggregates) with an optional trailing `WHERE`, e.g. `MATCH ... WITH n, count(*) AS c WHERE c > 1`.
- **`OPTIONAL MATCH`** — left join; unmatched rows are kept with the clause's new variables null.
- **`UNWIND <list> AS x`** — expand a list into rows.
- **`EXISTS { [MATCH] pattern [WHERE ...] }`** — existential subquery predicate, and `NOT EXISTS`.
- **`CASE`** — both `CASE x WHEN v THEN ...` and `CASE WHEN cond THEN ...` forms.
- **Map projections** — `n { .prop, key: expr }`.
- **`ORDER BY`**, **`SKIP`**, **`LIMIT`** (on both `WITH` and `RETURN`).
- **Comments** — line (`//`) and block (`/* ... */`).
- **Negative integer literals** — `-10`.

Write clauses (`CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE`) are intentionally unsupported.

## API

- `parse` — parse a query string into a `Query` AST.
- `GraphProvider` — implement this for your graph to make it queryable.
- `execute` — run a parsed query, returning a `ResultSet`.
- `run_query` — parse + execute + format in one call.
- `OutputFormat` / `CypherValue` — result formatting and values.
- `ast` — the AST types; `CypherError` — lexing/parsing/execution errors with a source position.

## Roadmap

Planned syntax additions (multi-label `AND`, floats/arithmetic, `=~` regex, parameters, `UNION`,
relationship-variable `RETURN`, …) and their prioritized implementation plans are in
[docs/ROADMAP.md](docs/ROADMAP.md) (with a quick [docs/CHECKLIST.md](docs/CHECKLIST.md)).

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) first. All contributors
must sign the Shopify Contributor License Agreement (a bot will prompt you on your first pull
request), and participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).

## License

This project is maintained by [Shopify](https://shopify.com) and released under the
[MIT License](LICENSE.md).
