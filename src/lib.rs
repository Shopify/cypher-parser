//! A hand-written lexer, recursive-descent parser, and AST for a practical subset of the
//! [Cypher](https://opencypher.org/) graph query language.
//!
//! This crate is dependency-free and execution-agnostic: it turns a query string into a [`Query`]
//! AST (or a positioned [`CypherError`]) and leaves evaluation to the caller. It is intended for
//! read-only, introspection-style queries.
//!
//! # Supported subset
//!
//! - `MATCH` with node patterns `(v:Label {prop: value})` — labels may be a disjunction
//!   (`(v:Class|Module)` matches a node with **any** of the listed labels) — and relationship
//!   patterns `-[:TYPE]->`, `<-[:TYPE]-`, `-[:TYPE]-`, including variable-length
//!   `-[:TYPE*min..max]->`.
//! - `WHERE` with `=`, `<>`, `<`, `<=`, `>`, `>=`, `CONTAINS`, `STARTS WITH`, `ENDS WITH`, combined
//!   with `AND`, `OR`, `NOT`.
//! - `RETURN` with `DISTINCT`, `AS` aliases, and the aggregates `count`, `collect`, `min`, `max`,
//!   `sum`, `avg`.
//! - `ORDER BY`, `SKIP`, `LIMIT`.
//!
//! Write clauses (`CREATE`, `MERGE`, `SET`, `DELETE`, …) are intentionally not supported.
//!
//! # Example
//!
//! ```
//! use cypher_parser::parse;
//!
//! let query = parse("MATCH (c:Class)-[:INHERITS*1..]->(p:Class {name: 'Base'}) RETURN c.name").unwrap();
//! assert_eq!(query.patterns.len(), 1);
//! assert_eq!(query.return_clause.items.len(), 1);
//! ```

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;

pub use ast::Query;
pub use error::CypherError;
pub use parser::parse;
