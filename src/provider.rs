//! The [`GraphProvider`] trait: the read-only property-graph interface the [`crate::executor`]
//! runs queries against. Implement it for your own graph or data structure to make it queryable
//! with this crate's Cypher subset.
//!
//! The model is the standard Cypher property graph: nodes carry one or more **labels** and scalar
//! **properties**, and are connected by typed, directed **relationships**. The executor only ever
//! reads through this trait, so any backend that can answer these questions can be queried.

use std::hash::Hash;

use crate::value::CypherValue;

/// A read-only property-graph data source.
///
/// All methods are read-only. Node identity is an associated type so implementers can use whatever
/// handle is natural (an index, an interned id, an enum, …), as long as it is cheap to copy, equate,
/// and hash.
pub trait GraphProvider {
    /// A lightweight handle identifying a node.
    type NodeId: Copy + Eq + Hash;

    /// Returns all nodes matching **any** of the given labels. An empty slice matches every node.
    fn scan(&self, labels: &[String]) -> Vec<Self::NodeId>;

    /// Returns whether `node` has the given label.
    fn matches_label(&self, node: Self::NodeId, label: &str) -> bool;

    /// Returns the names of all relationship types this graph exposes. Used to expand untyped
    /// relationship patterns (`-->`, `-[]->`) and to validate relationship-type names in queries.
    fn relationship_types(&self) -> Vec<String>;

    /// Returns the outgoing neighbours of `node` along the relationship type `rel_type`.
    fn expand(&self, node: Self::NodeId, rel_type: &str) -> Vec<Self::NodeId>;

    /// Returns the nodes that may have an outgoing `rel_type` edge. Used to build reverse adjacency
    /// for incoming and undirected traversal. Returning all nodes is always correct (if less
    /// efficient); returning a tighter set is an optimization.
    fn rel_sources(&self, rel_type: &str) -> Vec<Self::NodeId>;

    /// Returns the value of property `prop` on `node`, or [`CypherValue::Null`] if it is absent.
    fn property(&self, node: Self::NodeId, prop: &str) -> CypherValue;

    /// Returns a stable, opaque identifier for `node`, carried through to returned nodes so
    /// consumers can map a result back to their own object.
    ///
    /// The crate never interprets this string — it only compares it for identity (so distinct
    /// nodes get distinct ids) and never includes it in table or JSON output. Encode whatever the
    /// consumer needs to round-trip a node (for example a type tag plus a primary key).
    fn node_id(&self, node: Self::NodeId) -> String;

    /// Returns the node's primary label, used when a bound node is returned directly.
    fn label(&self, node: Self::NodeId) -> String;

    /// Returns the node's display name, used when a bound node is returned directly.
    fn name(&self, node: Self::NodeId) -> String;
}
