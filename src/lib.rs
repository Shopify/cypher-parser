//! A hand-written lexer, recursive-descent parser, and tree-walking executor for a practical subset
//! of the [Cypher](https://opencypher.org/) graph query language.
//!
//! The crate has two layers:
//!
//! 1. **Parsing** ([`parse`]) turns a query string into a [`Query`] AST (or a positioned
//!    [`CypherError`]). This layer is execution-agnostic.
//! 2. **Execution** ([`execute`] / [`run_query`]) evaluates a parsed query against any backend that
//!    implements the [`GraphProvider`] trait, producing a [`ResultSet`] that can be rendered as a
//!    table or JSON via [`OutputFormat`]. Implement [`GraphProvider`] for your own graph to make it
//!    queryable — the executor is generic and reads the graph only through that trait.
//!
//! It targets read-only, introspection-style queries.
//!
//! # Supported subset
//!
//! - `MATCH` with node patterns `(v:Label {prop: value})` — labels may be a disjunction
//!   (`(v:Class|Module)` matches a node with **any** of the listed labels) — and relationship
//!   patterns `-[:TYPE]->`, `<-[:TYPE]-`, `-[:TYPE]-`, including variable-length
//!   `-[:TYPE*min..max]->`.
//! - `WHERE` with `=`, `<>`, `<`, `<=`, `>`, `>=`, `CONTAINS`, `STARTS WITH`, `ENDS WITH`, `IN`
//!   (with `[...]` list literals), `IS NULL` / `IS NOT NULL`, combined with `AND`, `OR`, `NOT`.
//! - Scalar functions `toLower`, `toUpper`, `size`, `coalesce`, `labels`.
//! - `RETURN` with `DISTINCT`, `*` (all bound variables), `AS` aliases, and the aggregates `count`,
//!   `collect`, `min`, `max`, `sum`, `avg`.
//! - `WITH` to chain clauses: project (with the same features as `RETURN`, including aggregates)
//!   into a new set of bindings, with an optional trailing `WHERE` — enabling post-aggregation
//!   filtering such as `MATCH ... WITH n, count(*) AS c WHERE c > 1 RETURN ...`.
//! - `OPTIONAL MATCH` (left join: unmatched rows are kept with the clause's new variables null).
//! - `EXISTS { [MATCH] pattern [WHERE ...] }` existential subquery predicate (and `NOT EXISTS`).
//! - `ORDER BY`, `SKIP`, `LIMIT` (on both `WITH` and `RETURN`).
//!
//! Write clauses (`CREATE`, `MERGE`, `SET`, `DELETE`, …) are intentionally not supported.
//!
//! # Example
//!
//! ```
//! use cypher_parser::parse;
//!
//! let query = parse("MATCH (c:Class)-[:INHERITS*1..]->(p:Class {name: 'Base'}) RETURN c.name").unwrap();
//! assert_eq!(query.clauses.len(), 1);
//! assert_eq!(query.result.items.len(), 1);
//! ```
//!
//! To execute queries, implement [`GraphProvider`] for your data and call [`run_query`] or
//! [`execute`].

pub mod ast;
pub mod error;
pub mod executor;
pub mod format;
pub mod lexer;
pub mod parser;
pub mod provider;
pub mod value;

pub use ast::Query;
pub use error::CypherError;
pub use executor::{ResultSet, execute};
pub use format::OutputFormat;
pub use parser::parse;
pub use provider::GraphProvider;
pub use value::CypherValue;

/// Parses and executes a query against a [`GraphProvider`], returning the formatted output.
///
/// # Errors
///
/// Returns a [`CypherError`] if the query cannot be parsed or executed.
pub fn run_query<G: GraphProvider>(
    graph: &G,
    query: &str,
    output_format: OutputFormat,
) -> Result<String, CypherError> {
    let parsed = parser::parse(query)?;
    let result = executor::execute(graph, &parsed)?;
    Ok(format::format(&result, output_format))
}
