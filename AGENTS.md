# AGENTS.md

Guidance for AI agents (and humans) working on `cypher-parser`.

## What this is

A small, **dependency-free** lexer + recursive-descent parser + tree-walking executor for a
practical **read-only** subset of the Cypher graph query language. The executor is generic over a
`GraphProvider` trait, so it never knows about any concrete graph. Published to crates.io; the main
consumer is `rubydex` (which implements `GraphProvider` for its code graph).

Two layers: **parsing** (`parse` -> `Query` AST) and **execution** (`execute` / `run_query` against
a `GraphProvider`, producing a `ResultSet` renderable as a table or JSON).

## Source layout (`src/`)

| File | Responsibility |
|------|----------------|
| `lexer.rs` | `tokenize(&str) -> Vec<Token>`; `TokenKind`. Skips `//` and `/* */` comments. |
| `ast.rs` | AST. `Query { clauses: Vec<Clause>, result: Projection }`; `Clause::{Match,Unwind,With}`; `Expr`; `Projection`; `PathPattern`; etc. |
| `parser.rs` | Recursive-descent parser producing `Query`. |
| `executor.rs` | `execute<G: GraphProvider>(&G, &Query) -> ResultSet`. Clause pipeline, pattern matching, projection, aggregation. |
| `provider.rs` | The `GraphProvider` trait — the read-only property-graph interface. |
| `value.rs` | `CypherValue` (Null/Bool/Int/Str/Node/List/Map), orderability, table/JSON rendering. |
| `format.rs` | `OutputFormat`, table and JSON formatting. |
| `error.rs` | `CypherError::{Syntax{message,position}, Execution{message}}`. |

Tests live in `tests/`: `parser.rs` (parse-only), `executor.rs` (against an in-memory provider),
`tck.rs` (hand-ported openCypher TCK scenarios), `scale.rs` (large synthetic corpus + parity),
`expand_in.rs` (spy provider for the incoming-traversal hook).

## Commands

```
cargo build
cargo test                       # unit + integration + doctests
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

All four must pass before a PR merges. CI runs them as the `test` job (required status check).

## Supported subset (keep this and README/`src/lib.rs` docs in sync)

- `MATCH` (incl. `OPTIONAL MATCH`): node patterns with label disjunction `(v:A|B)` and inline
  `{prop: value}`; relationship patterns with direction, type lists, and variable length `*min..max`.
- `WHERE`: `= <> < <= > >= CONTAINS STARTS WITH ENDS WITH IN IS [NOT] NULL`, `AND/OR/NOT`,
  `EXISTS { [MATCH] pattern [WHERE ...] }` (and `NOT EXISTS`).
- `WITH` (clause chaining, post-aggregation filtering), `UNWIND <list> AS x`.
- `RETURN`/`WITH` projections: `DISTINCT`, `*`, `AS` aliases; aggregates `count collect min max sum avg`;
  scalar functions `toLower toUpper size coalesce labels`; `CASE`; map projections `n { .prop, k: expr }`.
- `ORDER BY` / `SKIP` / `LIMIT` (on both `WITH` and `RETURN`).
- Literals: string, integer (incl. negative `-10`), bool, null, lists `[...]`, maps via projection.
- Comments `//` and `/* */`.

**Out of scope (reject / do not implement):** write clauses — `CREATE`, `MERGE`, `SET`, `DELETE`,
`REMOVE`, `DETACH DELETE`.

## Conventions & gotchas

- **Dependency-free.** Do not add a runtime dependency without explicit discussion (e.g. `regex`).
- **Errors:** `CypherError::syntax(msg, byte_pos)` in lexer/parser; `CypherError::execution(msg)` in
  the executor. Keep messages specific.
- **`CypherValue` derives `Eq + Hash`** (used for grouping/`DISTINCT`). Ordering is
  `CypherValue::total_cmp`, aligned with openCypher orderability:
  `Map < Node < List < String < Boolean < Number < null` (null sorts last, strings before numbers).
  Nodes are ordered by **name**, not identity (deliberate deviation). Comparisons (`<`, `>`, …)
  return false across differing types (`compare_values` guards this); only `ORDER BY` and `min`/`max`
  use cross-type orderability.
- **Keep the graph behind `GraphProvider`.** The executor reads the graph only through the trait.
  Node identity is `GraphProvider::node_id` (stable, opaque; backs node equality and is never
  rendered). `expand_in` is an **optional, defaulted** hook for targeted incoming-neighbour lookup;
  when a provider returns `None` the executor falls back to building reverse adjacency.
- **Determinism:** results feed CLIs/tests; keep ordering deterministic (`RETURN *` uses pattern
  declaration order; dedup/sort where needed).
- **Parser requires a leading reading clause** (`MATCH`/`UNWIND`/`WITH`); `RETURN 1` alone errors.
- **Not supported yet:** floats/arithmetic, multi-label AND `(:A:B)`, `=~` regex, parameters `$name`,
  `UNION`, and **relationship-variable `RETURN`** (relationship variables are not bound; that needs a
  breaking `GraphProvider` `RelId` extension). `i64::MIN` literal is unsupported (lexer overflow).

## Adding syntax

Touches `lexer.rs` (if new tokens), `ast.rs`, `parser.rs`, `executor.rs`, plus tests
(`tests/parser.rs` + `tests/executor.rs`, incl. an error case) and docs (README supported-subset and
`src/lib.rs` crate docs). The `GraphProvider` trait should only change if a construct genuinely needs
a new graph capability.

## Versioning

Semver, `0.x` (second component acts as the major):
- **Breaking → minor bump** (`0.x.0`). In Rust this includes adding a public enum variant, a public
  struct field, or a trait method **without** a default. Most AST additions are therefore breaking.
- **Additive / perf / behavior-preserving → patch** (`0.x.y`) when there's no public API change.
- A defaulted trait method (like `expand_in`) is **non-breaking**.

## Release process

Releases use crates.io **Trusted Publishing** (OIDC — no stored token), gated by a manual approval.

1. Bump `version` in `Cargo.toml`; land it via PR.
2. Tag and push: `git tag -a vX.Y.Z -m "cypher-parser vX.Y.Z" && git push origin vX.Y.Z`.
3. `.github/workflows/release.yml` (triggers on `v*` tags) runs in the `release` GitHub
   **environment**, which **requires manual approval** — approve the pending deployment. It then
   `cargo publish`es via OIDC and creates a **GitHub Release** with auto-generated, label-categorized
   notes.
4. Verify on crates.io.

GitHub Actions are **pinned to commit SHAs** (Dependabot bumps them). The crates.io Trusted Publisher
must be registered (repo `Shopify/cypher-parser`, workflow `release.yml`, environment `release`).

## PR workflow (enforced)

`main` is protected by a ruleset: **a PR is required**, the **`test`** check must pass, and
direct/force pushes are blocked. 0 required approvals (self-merge is allowed for now).

- Work on a branch -> open a PR -> CI green -> merge.
- **Merge commits only** (squash and rebase are disabled); head branches auto-delete on merge;
  auto-merge is available.
- **Label every PR** (`breaking`, `feature`, `performance`, `fix`, `documentation`) — labels drive the
  release-note sections defined in `.github/release.yml`.

## `docs/` is intentionally local — DO NOT COMMIT IT

`docs/ROADMAP.md` and `docs/CHECKLIST.md` hold the gap roadmap and progress checklist. The maintainer
keeps them **local and untracked on purpose**. Never `git add` them, and never commit the `docs/`
directory. When staging, add specific paths (`src/`, `tests/`, `Cargo.toml`, `README.md`, …) rather
than `git add -A`.
