# cypher-parser

A small, dependency-free lexer and recursive-descent parser for a practical subset of the
[Cypher](https://opencypher.org/) graph query language.

It turns a query string into an AST (or a positioned error) and leaves execution to the caller, so
it can front any graph or in-memory data structure. It targets read-only, introspection-style
queries.

## Usage

```rust
use cypher_parser::parse;

let query = parse(
    "MATCH (c:Class)-[:INHERITS*1..]->(p:Class {name: 'Base'}) \
     WHERE c.name CONTAINS 'Service' \
     RETURN c.name, count(*) AS total \
     ORDER BY total DESC LIMIT 10",
)?;

for item in &query.return_clause.items {
    println!("{}", item.column_name());
}
# Ok::<(), cypher_parser::CypherError>(())
```

## Supported subset

- **`MATCH`** — node patterns `(v:Label {prop: value})` with label disjunction (`(v:A|B)`); relationship
  patterns `-[:TYPE]->`, `<-[:TYPE]-`, `-[:TYPE]-`, including variable-length `-[:TYPE*min..max]->`.
- **`WHERE`** — `=`, `<>`, `<`, `<=`, `>`, `>=`, `CONTAINS`, `STARTS WITH`, `ENDS WITH`, combined with
  `AND`, `OR`, `NOT`.
- **`RETURN`** — `DISTINCT`, `AS` aliases, and the aggregates `count`, `collect`, `min`, `max`, `sum`,
  `avg`.
- **`ORDER BY`**, **`SKIP`**, **`LIMIT`**.

Write clauses (`CREATE`, `MERGE`, `SET`, `DELETE`, `REMOVE`) are intentionally unsupported.

## API

- [`parse`] — parse a query string into a [`ast::Query`].
- [`ast`] — the AST types.
- [`error::CypherError`] — lexing/parsing errors, with a byte position into the source.

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) first. All contributors
must sign the Shopify Contributor License Agreement (a bot will prompt you on your first pull
request), and participation is governed by our [Code of Conduct](CODE_OF_CONDUCT.md).

## License

This project is maintained by [Shopify](https://shopify.com) and released under the
[MIT License](LICENSE.md).
