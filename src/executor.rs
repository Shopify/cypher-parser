use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::HashSet;
use std::rc::Rc;

use crate::ast::{
    AggFn, Clause, CmpOp, Direction, Expr, Literal, MatchClause, NodePattern, OrderItem,
    PathPattern, Projection, Query, ReturnItem, UnwindClause, WithClause,
};
use crate::error::CypherError;
use crate::provider::GraphProvider;
use crate::value::CypherValue;

/// The tabular result of executing a query.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultSet {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CypherValue>>,
}

/// A bound variable: either a graph node (which keeps its identity for pattern chaining) or a
/// scalar/computed value (produced by a `WITH` projection).
enum Binding<N> {
    Node(N),
    Value(CypherValue),
}

impl<N: Copy> Clone for Binding<N> {
    fn clone(&self) -> Self {
        match self {
            Binding::Node(n) => Binding::Node(*n),
            Binding::Value(v) => Binding::Value(v.clone()),
        }
    }
}

/// A single binding row: maps variable names to their bindings.
type Row<N> = HashMap<String, Binding<N>>;

/// A binding row paired with the node most recently matched in the path being expanded.
type Working<N> = Vec<(Row<N>, N)>;

/// A projected row: the new bindings carried to the next clause, plus the aligned output values.
struct Projected<N> {
    bindings: Row<N>,
    values: Vec<CypherValue>,
}

/// Output column names paired with the binding rows a `WITH` clause carries forward.
type WithOutput<N> = (Vec<String>, Vec<Row<N>>);

/// Output column names paired with the projected rows of a projection.
type ProjectedRows<N> = (Vec<String>, Vec<Projected<N>>);

/// Lazily-built reverse adjacency (target -> sources) per relationship type.
type ReverseCache<N> = RefCell<HashMap<String, HashMap<N, Vec<N>>>>;

/// Executes a parsed query against any [`GraphProvider`].
///
/// # Errors
///
/// Returns a [`CypherError::Execution`] for unknown relationship types, aggregates used in `WHERE`,
/// or `ORDER BY` expressions that cannot be resolved against the projection.
pub fn execute<G: GraphProvider>(graph: &G, query: &Query) -> Result<ResultSet, CypherError> {
    let executor = Executor {
        graph,
        reverse_cache: RefCell::new(HashMap::new()),
        in_hoist: RefCell::new(HashMap::new()),
        pushdown: RefCell::new(HashMap::new()),
    };
    executor.run(query)
}

struct Executor<'a, G: GraphProvider> {
    graph: &'a G,
    /// Lazily-built reverse adjacency (target -> sources) per relationship type, for incoming and
    /// undirected traversal. Behind a `RefCell` so pattern matching can run through `&self` (needed
    /// to evaluate `EXISTS` predicates during expression evaluation).
    reverse_cache: ReverseCache<G::NodeId>,
    /// Per-filter-pass memo of loop-invariant `IN` right-hand-side lists, keyed by the RHS
    /// expression's address, so `x IN <constant list>` is an O(1) `HashSet` lookup instead of an
    /// O(list) scan re-evaluated per row. Populated for the duration of a single `filter_rows` pass.
    in_hoist: RefCell<HashMap<usize, Rc<HashSet<CypherValue>>>>,
    /// Single-variable WHERE predicates pushed down into candidate generation, keyed by variable
    /// name, so scans prune before pattern expansion. Set for the duration of a MATCH clause's
    /// pattern expansion; the full WHERE is still applied afterwards.
    pushdown: RefCell<HashMap<String, Vec<Expr>>>,
}

impl<G: GraphProvider> Executor<'_, G> {
    fn run(&self, query: &Query) -> Result<ResultSet, CypherError> {
        let mut rows: Vec<Row<G::NodeId>> = vec![Row::new()];
        let mut scope: Vec<String> = Vec::new();

        for clause in &query.clauses {
            match clause {
                Clause::Match(m) if m.optional => {
                    rows = self.run_optional_match(rows, m, &mut scope)?;
                }
                Clause::Match(m) => rows = self.run_match(rows, m, &mut scope)?,
                Clause::Unwind(u) => {
                    rows = self.run_unwind(rows, u)?;
                    if !scope.contains(&u.var) {
                        scope.push(u.var.clone());
                    }
                }
                Clause::With(w) => {
                    let (columns, rows_after) = self.run_with(&rows, w, &scope)?;
                    rows = rows_after;
                    scope = columns;
                }
            }
        }

        // Terminal RETURN.
        let (columns, projected) = self.project_rows(&query.result, &rows, &scope)?;
        let projected = finalize(&query.result, &columns, projected)?;
        Ok(ResultSet {
            columns,
            rows: projected.into_iter().map(|p| p.values).collect(),
        })
    }

    fn run_match(
        &self,
        mut rows: Vec<Row<G::NodeId>>,
        clause: &MatchClause,
        scope: &mut Vec<String>,
    ) -> Result<Vec<Row<G::NodeId>>, CypherError> {
        // Variables that are constant across the whole pass: if the clause's input is a single row
        // (e.g. right after `WITH collect(...) AS used`), every variable it binds has the same value
        // in every row produced below, so `x IN <those vars>` right-hand sides can be hoisted.
        let constant_vars: HashSet<String> = if rows.len() == 1 {
            rows[0].keys().cloned().collect()
        } else {
            HashSet::new()
        };

        // Push single-variable predicates down into candidate generation so scans prune before
        // expansion. The full WHERE still runs afterwards, so this is a safe pre-filter.
        *self.pushdown.borrow_mut() = build_pushdown(&clause.where_clause);
        for pattern in &clause.patterns {
            rows = self.eval_pattern(rows, pattern)?;
            add_pattern_vars(scope, pattern);
        }
        self.pushdown.borrow_mut().clear();

        if let Some(predicate) = &clause.where_clause {
            rows = self.filter_rows_hoisted(rows, predicate, &constant_vars)?;
        }
        Ok(rows)
    }

    /// Whether a candidate node satisfies the single-variable predicates pushed down for its
    /// variable. Best-effort: evaluation errors are ignored here (the full WHERE runs later and
    /// will surface them), so a node is only pruned when a pushed predicate is definitively false.
    fn node_passes_pushdown(&self, var: &Option<String>, node: G::NodeId) -> bool {
        let Some(name) = var else {
            return true;
        };
        let preds = match self.pushdown.borrow().get(name) {
            Some(preds) => preds.clone(),
            None => return true,
        };
        let mut row = Row::new();
        row.insert(name.clone(), Binding::Node(node));
        for predicate in &preds {
            if let Ok(value) = self.eval_expr(&row, predicate)
                && !value.is_truthy()
            {
                return false;
            }
        }
        true
    }

    /// Filters rows by a predicate, first hoisting any loop-invariant `IN` right-hand side (one
    /// whose variables are all constant across the pass) into a `HashSet` evaluated once.
    fn filter_rows_hoisted(
        &self,
        rows: Vec<Row<G::NodeId>>,
        predicate: &Expr,
        constant_vars: &HashSet<String>,
    ) -> Result<Vec<Row<G::NodeId>>, CypherError> {
        if constant_vars.is_empty() || rows.is_empty() {
            return self.filter_rows(rows, predicate);
        }

        // The constant variables have identical values in every row, so evaluate hoistable RHS
        // lists against the first row.
        self.in_hoist.borrow_mut().clear();
        self.build_in_hoist(predicate, constant_vars, &rows[0])?;
        let result = self.filter_rows(rows, predicate);
        self.in_hoist.borrow_mut().clear();
        result
    }

    /// Walks a predicate and, for each `expr IN <list>` whose right-hand side references only
    /// constant variables, evaluates the list once and stores it as a `HashSet` keyed by the RHS
    /// expression's address.
    fn build_in_hoist(
        &self,
        expr: &Expr,
        constant_vars: &HashSet<String>,
        sample: &Row<G::NodeId>,
    ) -> Result<(), CypherError> {
        match expr {
            Expr::Compare(left, CmpOp::In, right) => {
                self.build_in_hoist(left, constant_vars, sample)?;
                if references_only(right, constant_vars)
                    && let CypherValue::List(items) = self.eval_expr(sample, right)?
                {
                    let set: HashSet<CypherValue> = items.into_iter().collect();
                    let key = std::ptr::from_ref::<Expr>(right) as usize;
                    self.in_hoist.borrow_mut().insert(key, Rc::new(set));
                }
            }
            Expr::Not(inner) | Expr::IsNull(inner, _) => {
                self.build_in_hoist(inner, constant_vars, sample)?;
            }
            Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
                self.build_in_hoist(a, constant_vars, sample)?;
                self.build_in_hoist(b, constant_vars, sample)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Runs `UNWIND <expr> AS var`: for each input row, evaluates the list and emits one output row
    /// per element with `var` bound to it. A null list yields no rows; a non-list is an error.
    fn run_unwind(
        &self,
        rows: Vec<Row<G::NodeId>>,
        clause: &UnwindClause,
    ) -> Result<Vec<Row<G::NodeId>>, CypherError> {
        let mut output = Vec::new();
        for row in rows {
            match self.eval_expr(&row, &clause.expr)? {
                CypherValue::List(items) => {
                    for item in items {
                        let mut new_row = row.clone();
                        new_row.insert(clause.var.clone(), Binding::Value(item));
                        output.push(new_row);
                    }
                }
                CypherValue::Null => {}
                _ => {
                    return Err(CypherError::execution("UNWIND expects a list"));
                }
            }
        }
        Ok(output)
    }

    /// Runs `OPTIONAL MATCH`: a left join. For each input row, the patterns (and the clause's
    /// `WHERE`) are matched independently; if nothing matches, the input row is kept with the
    /// clause's newly introduced variables bound to null.
    fn run_optional_match(
        &self,
        rows: Vec<Row<G::NodeId>>,
        clause: &MatchClause,
        scope: &mut Vec<String>,
    ) -> Result<Vec<Row<G::NodeId>>, CypherError> {
        // Variables introduced by this clause that are not already in scope; these are nulled out
        // for input rows that produce no match.
        let mut new_vars: Vec<String> = Vec::new();
        for pattern in &clause.patterns {
            add_pattern_vars(&mut new_vars, pattern);
        }
        new_vars.retain(|var| !scope.contains(var));

        let mut output = Vec::with_capacity(rows.len());
        for row in rows {
            let mut matched = vec![row.clone()];
            for pattern in &clause.patterns {
                matched = self.eval_pattern(matched, pattern)?;
            }
            if let Some(predicate) = &clause.where_clause {
                matched = self.filter_rows(matched, predicate)?;
            }

            if matched.is_empty() {
                let mut null_row = row;
                for var in &new_vars {
                    null_row.insert(var.clone(), Binding::Value(CypherValue::Null));
                }
                output.push(null_row);
            } else {
                output.extend(matched);
            }
        }

        for pattern in &clause.patterns {
            add_pattern_vars(scope, pattern);
        }
        Ok(output)
    }

    fn run_with(
        &self,
        rows: &[Row<G::NodeId>],
        clause: &WithClause,
        scope: &[String],
    ) -> Result<WithOutput<G::NodeId>, CypherError> {
        let (columns, projected) = self.project_rows(&clause.projection, rows, scope)?;
        let projected = finalize(&clause.projection, &columns, projected)?;

        // `WITH ... WHERE` filters the projected bindings.
        let mut new_rows = Vec::with_capacity(projected.len());
        for p in projected {
            if let Some(predicate) = &clause.where_clause
                && !self.eval_expr(&p.bindings, predicate)?.is_truthy()
            {
                continue;
            }
            new_rows.push(p.bindings);
        }
        Ok((columns, new_rows))
    }

    fn filter_rows(
        &self,
        rows: Vec<Row<G::NodeId>>,
        predicate: &Expr,
    ) -> Result<Vec<Row<G::NodeId>>, CypherError> {
        let mut filtered = Vec::with_capacity(rows.len());
        for row in rows {
            if self.eval_expr(&row, predicate)?.is_truthy() {
                filtered.push(row);
            }
        }
        Ok(filtered)
    }

    // ---- Pattern matching ------------------------------------------------

    fn eval_pattern(
        &self,
        base: Vec<Row<G::NodeId>>,
        pattern: &PathPattern,
    ) -> Result<Vec<Row<G::NodeId>>, CypherError> {
        // Plan the traversal so it starts from an already-bound endpoint when possible: a
        // forward-written `(x)-[:R]->(d)` with `d` bound is executed as `(d)<-[:R]-(x)`, walking
        // through the incoming cache instead of scanning every node as `x`. Reversal yields the
        // identical set of variable bindings; it only changes traversal direction/cost.
        let planned = plan_pattern(pattern, base.first());
        let pattern = &planned;

        let mut working: Working<G::NodeId> = Vec::new();

        for row in base {
            for node in self.candidates_for_node(&row, &pattern.start) {
                let mut new_row = row.clone();
                if let Some(var) = &pattern.start.var {
                    new_row.insert(var.clone(), Binding::Node(node));
                }
                working.push((new_row, node));
            }
        }

        for (rel, node) in &pattern.rest {
            working = self.expand_step(working, rel, node)?;
        }

        Ok(working.into_iter().map(|(row, _)| row).collect())
    }

    fn candidates_for_node(&self, row: &Row<G::NodeId>, pattern: &NodePattern) -> Vec<G::NodeId> {
        if let Some(var) = &pattern.var
            && let Some(binding) = row.get(var)
        {
            return match binding {
                Binding::Node(existing) if self.node_matches(*existing, pattern) => vec![*existing],
                _ => Vec::new(),
            };
        }

        self.graph
            .scan(&pattern.labels)
            .into_iter()
            .filter(|node| self.props_match(*node, pattern))
            .filter(|node| self.node_passes_pushdown(&pattern.var, *node))
            .collect()
    }

    fn expand_step(
        &self,
        working: Working<G::NodeId>,
        rel: &crate::ast::RelPattern,
        node: &NodePattern,
    ) -> Result<Working<G::NodeId>, CypherError> {
        let rel_types = self.resolve_rel_types(rel)?;
        let mut next = Vec::new();

        for (row, current) in working {
            let targets =
                self.step_targets(current, &rel_types, rel.direction, rel.length.as_ref());
            for target in targets {
                if !self.node_matches(target, node) {
                    continue;
                }
                if !self.node_passes_pushdown(&node.var, target) {
                    continue;
                }
                // If the target variable is already bound, it must be the same node.
                if let Some(var) = &node.var
                    && let Some(existing) = row.get(var)
                {
                    match existing {
                        Binding::Node(n) if *n == target => {}
                        _ => continue,
                    }
                }
                let mut new_row = row.clone();
                if let Some(var) = &node.var {
                    new_row.insert(var.clone(), Binding::Node(target));
                }
                next.push((new_row, target));
            }
        }

        Ok(next)
    }

    fn step_targets(
        &self,
        node: G::NodeId,
        rel_types: &[String],
        direction: Direction,
        length: Option<&crate::ast::VarLength>,
    ) -> Vec<G::NodeId> {
        if let Some(var_length) = length {
            return self.var_length_targets(node, rel_types, direction, var_length);
        }

        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for rel in rel_types {
            for target in self.step_once(node, rel, direction) {
                if seen.insert(target) {
                    targets.push(target);
                }
            }
        }
        targets
    }

    fn step_once(&self, node: G::NodeId, rel: &str, direction: Direction) -> Vec<G::NodeId> {
        match direction {
            Direction::Outgoing => self.graph.expand(node, rel),
            Direction::Incoming => self.incoming(node, rel),
            Direction::Both => {
                let mut seen = HashSet::new();
                let mut targets = Vec::new();
                for target in self
                    .graph
                    .expand(node, rel)
                    .into_iter()
                    .chain(self.incoming(node, rel))
                {
                    if seen.insert(target) {
                        targets.push(target);
                    }
                }
                targets
            }
        }
    }

    fn incoming(&self, node: G::NodeId, rel: &str) -> Vec<G::NodeId> {
        if !self.reverse_cache.borrow().contains_key(rel) {
            let mut reverse: HashMap<G::NodeId, Vec<G::NodeId>> = HashMap::new();
            for source in self.graph.rel_sources(rel) {
                for target in self.graph.expand(source, rel) {
                    reverse.entry(target).or_default().push(source);
                }
            }
            self.reverse_cache
                .borrow_mut()
                .insert(rel.to_string(), reverse);
        }

        self.reverse_cache
            .borrow()
            .get(rel)
            .and_then(|reverse| reverse.get(&node))
            .cloned()
            .unwrap_or_default()
    }

    fn var_length_targets(
        &self,
        start: G::NodeId,
        rel_types: &[String],
        direction: Direction,
        var_length: &crate::ast::VarLength,
    ) -> Vec<G::NodeId> {
        let max = var_length.max.unwrap_or(u32::MAX);
        let mut results = Vec::new();
        let mut result_seen = HashSet::new();

        if var_length.min == 0 {
            results.push(start);
            result_seen.insert(start);
        }

        let mut visited = HashSet::new();
        visited.insert(start);
        let mut frontier = vec![start];
        let mut depth = 0u32;

        while depth < max && !frontier.is_empty() {
            depth += 1;
            let mut next = Vec::new();
            for node in &frontier {
                for rel in rel_types {
                    for target in self.step_once(*node, rel, direction) {
                        if visited.insert(target) {
                            next.push(target);
                            if depth >= var_length.min && result_seen.insert(target) {
                                results.push(target);
                            }
                        }
                    }
                }
            }
            frontier = next;
        }

        results
    }

    fn node_matches(&self, node: G::NodeId, pattern: &NodePattern) -> bool {
        if !self.matches_labels(node, &pattern.labels) {
            return false;
        }
        self.props_match(node, pattern)
    }

    /// A node matches an empty label set, or any one of the listed labels (label disjunction).
    fn matches_labels(&self, node: G::NodeId, labels: &[String]) -> bool {
        labels.is_empty()
            || labels
                .iter()
                .any(|label| self.graph.matches_label(node, label))
    }

    fn props_match(&self, node: G::NodeId, pattern: &NodePattern) -> bool {
        pattern
            .props
            .iter()
            .all(|(key, literal)| self.graph.property(node, key) == literal_to_value(literal))
    }

    fn resolve_rel_types(&self, rel: &crate::ast::RelPattern) -> Result<Vec<String>, CypherError> {
        let known = self.graph.relationship_types();
        if rel.types.is_empty() {
            return Ok(known);
        }

        for name in &rel.types {
            if !known.iter().any(|known_type| known_type == name) {
                return Err(CypherError::execution(format!(
                    "unknown relationship type `{name}`"
                )));
            }
        }
        Ok(rel.types.clone())
    }

    // ---- Expression evaluation -------------------------------------------

    fn eval_expr(&self, row: &Row<G::NodeId>, expr: &Expr) -> Result<CypherValue, CypherError> {
        match expr {
            Expr::Literal(literal) => Ok(literal_to_value(literal)),
            Expr::Var(name) => Ok(match row.get(name) {
                Some(Binding::Node(node)) => self.node_value(*node),
                Some(Binding::Value(value)) => value.clone(),
                None => CypherValue::Null,
            }),
            Expr::Property(var, prop) => Ok(match row.get(var) {
                Some(Binding::Node(node)) => self.graph.property(*node, prop),
                _ => CypherValue::Null,
            }),
            Expr::Not(inner) => Ok(CypherValue::Bool(!self.eval_expr(row, inner)?.is_truthy())),
            Expr::And(a, b) => Ok(CypherValue::Bool(
                self.eval_expr(row, a)?.is_truthy() && self.eval_expr(row, b)?.is_truthy(),
            )),
            Expr::Or(a, b) => Ok(CypherValue::Bool(
                self.eval_expr(row, a)?.is_truthy() || self.eval_expr(row, b)?.is_truthy(),
            )),
            Expr::Compare(a, op, b) => {
                // Fast path: `x IN <hoisted constant list>` becomes an O(1) HashSet membership.
                if *op == CmpOp::In {
                    let key = std::ptr::from_ref::<Expr>(b) as usize;
                    let hoisted = self.in_hoist.borrow().get(&key).cloned();
                    if let Some(set) = hoisted {
                        let left = self.eval_expr(row, a)?;
                        let found = !matches!(left, CypherValue::Null) && set.contains(&left);
                        return Ok(CypherValue::Bool(found));
                    }
                }
                let left = self.eval_expr(row, a)?;
                let right = self.eval_expr(row, b)?;
                Ok(CypherValue::Bool(compare_values(&left, *op, &right)))
            }
            Expr::List(items) => {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(self.eval_expr(row, item)?);
                }
                Ok(CypherValue::List(values))
            }
            Expr::IsNull(inner, negated) => {
                let is_null = matches!(self.eval_expr(row, inner)?, CypherValue::Null);
                Ok(CypherValue::Bool(is_null ^ *negated))
            }
            Expr::Exists {
                patterns,
                where_clause,
            } => {
                let mut rows = vec![row.clone()];
                for pattern in patterns {
                    rows = self.eval_pattern(rows, pattern)?;
                }
                if let Some(predicate) = where_clause {
                    rows = self.filter_rows(rows, predicate)?;
                }
                Ok(CypherValue::Bool(!rows.is_empty()))
            }
            Expr::Case {
                operand,
                branches,
                default,
            } => self.eval_case(row, operand.as_deref(), branches, default.as_deref()),
            Expr::Function { name, args } => self.eval_function(row, name, args),
            Expr::MapProjection { var, entries } => self.eval_map_projection(row, var, entries),
            Expr::Aggregate { .. } => Err(CypherError::execution(
                "aggregate functions are only allowed in WITH or RETURN projections",
            )),
        }
    }

    fn eval_map_projection(
        &self,
        row: &Row<G::NodeId>,
        var: &str,
        entries: &[crate::ast::MapEntry],
    ) -> Result<CypherValue, CypherError> {
        // A map projection is defined over a node; on a null/non-node binding it yields null.
        let node = match row.get(var) {
            Some(Binding::Node(node)) => *node,
            _ => return Ok(CypherValue::Null),
        };

        let mut map = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                crate::ast::MapEntry::Property(prop) => {
                    map.push((prop.clone(), self.graph.property(node, prop)));
                }
                crate::ast::MapEntry::Literal(key, expr) => {
                    map.push((key.clone(), self.eval_expr(row, expr)?));
                }
            }
        }
        Ok(CypherValue::Map(map))
    }

    fn eval_case(
        &self,
        row: &Row<G::NodeId>,
        operand: Option<&Expr>,
        branches: &[(Expr, Expr)],
        default: Option<&Expr>,
    ) -> Result<CypherValue, CypherError> {
        if let Some(operand) = operand {
            // Simple form: compare the operand against each WHEN value.
            let subject = self.eval_expr(row, operand)?;
            for (when, then) in branches {
                let candidate = self.eval_expr(row, when)?;
                if values_equal(&subject, &candidate) {
                    return self.eval_expr(row, then);
                }
            }
        } else {
            // Generic form: the first truthy WHEN condition wins.
            for (when, then) in branches {
                if self.eval_expr(row, when)?.is_truthy() {
                    return self.eval_expr(row, then);
                }
            }
        }
        match default {
            Some(default) => self.eval_expr(row, default),
            None => Ok(CypherValue::Null),
        }
    }

    fn eval_function(
        &self,
        row: &Row<G::NodeId>,
        name: &str,
        args: &[Expr],
    ) -> Result<CypherValue, CypherError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "tolower" | "toupper" => {
                let arg = single_arg(name, args)?;
                Ok(match self.eval_expr(row, arg)? {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Str(s) => CypherValue::Str(if lower == "tolower" {
                        s.to_lowercase()
                    } else {
                        s.to_uppercase()
                    }),
                    _ => {
                        return Err(CypherError::execution(format!(
                            "`{name}` expects a string argument"
                        )));
                    }
                })
            }
            "size" => {
                let arg = single_arg(name, args)?;
                Ok(match self.eval_expr(row, arg)? {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Str(s) => {
                        CypherValue::Int(i64::try_from(s.chars().count()).unwrap_or(i64::MAX))
                    }
                    CypherValue::List(items) => {
                        CypherValue::Int(i64::try_from(items.len()).unwrap_or(i64::MAX))
                    }
                    _ => {
                        return Err(CypherError::execution(
                            "`size` expects a string or list argument",
                        ));
                    }
                })
            }
            "coalesce" => {
                if args.is_empty() {
                    return Err(CypherError::execution(
                        "`coalesce` requires at least one argument",
                    ));
                }
                for arg in args {
                    let value = self.eval_expr(row, arg)?;
                    if value != CypherValue::Null {
                        return Ok(value);
                    }
                }
                Ok(CypherValue::Null)
            }
            "labels" => {
                let arg = single_arg(name, args)?;
                Ok(match self.eval_expr(row, arg)? {
                    CypherValue::Null => CypherValue::Null,
                    CypherValue::Node { label, .. } => {
                        CypherValue::List(vec![CypherValue::Str(label)])
                    }
                    _ => {
                        return Err(CypherError::execution("`labels` expects a node argument"));
                    }
                })
            }
            _ => Err(CypherError::execution(format!("unknown function `{name}`"))),
        }
    }

    fn node_value(&self, node: G::NodeId) -> CypherValue {
        CypherValue::Node {
            id: self.graph.node_id(node),
            label: self.graph.label(node),
            name: self.graph.name(node),
        }
    }

    fn binding_value(&self, binding: &Binding<G::NodeId>) -> CypherValue {
        match binding {
            Binding::Node(node) => self.node_value(*node),
            Binding::Value(value) => value.clone(),
        }
    }

    // ---- Projection ------------------------------------------------------

    /// Projects the current rows through a `WITH` or `RETURN` projection, returning the output
    /// column names and one [`Projected`] per output row (bindings + aligned display values).
    /// Does not apply DISTINCT / ORDER BY / SKIP / LIMIT — see [`finalize`].
    fn project_rows(
        &self,
        projection: &Projection,
        rows: &[Row<G::NodeId>],
        scope: &[String],
    ) -> Result<ProjectedRows<G::NodeId>, CypherError> {
        if projection.star {
            return self.project_star(rows, scope);
        }

        let items = &projection.items;
        let columns = projection.column_names();

        let projected = if projection.has_aggregate() {
            self.project_aggregated(items, &columns, rows)?
        } else {
            self.project_simple(items, &columns, rows)?
        };

        Ok((columns, projected))
    }

    fn project_star(
        &self,
        rows: &[Row<G::NodeId>],
        scope: &[String],
    ) -> Result<ProjectedRows<G::NodeId>, CypherError> {
        if scope.is_empty() {
            return Err(CypherError::execution(
                "`*` requires at least one variable in scope",
            ));
        }

        let columns = scope.to_vec();
        let mut projected = Vec::with_capacity(rows.len());
        for row in rows {
            let mut bindings = Row::new();
            let mut values = Vec::with_capacity(columns.len());
            for name in &columns {
                let binding = row
                    .get(name)
                    .cloned()
                    .unwrap_or(Binding::Value(CypherValue::Null));
                values.push(self.binding_value(&binding));
                bindings.insert(name.clone(), binding);
            }
            projected.push(Projected { bindings, values });
        }
        Ok((columns, projected))
    }

    fn project_simple(
        &self,
        items: &[ReturnItem],
        columns: &[String],
        rows: &[Row<G::NodeId>],
    ) -> Result<Vec<Projected<G::NodeId>>, CypherError> {
        let mut output = Vec::with_capacity(rows.len());
        for row in rows {
            output.push(self.project_one(items, columns, row)?);
        }
        Ok(output)
    }

    /// Projects a single input row, preserving node identity for bare-variable items so the
    /// projected variable can still be used as a node by later pattern matching.
    fn project_one(
        &self,
        items: &[ReturnItem],
        columns: &[String],
        row: &Row<G::NodeId>,
    ) -> Result<Projected<G::NodeId>, CypherError> {
        let mut bindings = Row::new();
        let mut values = Vec::with_capacity(items.len());
        for (column, item) in columns.iter().zip(items) {
            let binding = self.project_binding(row, item)?;
            values.push(self.binding_value(&binding));
            bindings.insert(column.clone(), binding);
        }
        Ok(Projected { bindings, values })
    }

    fn project_binding(
        &self,
        row: &Row<G::NodeId>,
        item: &ReturnItem,
    ) -> Result<Binding<G::NodeId>, CypherError> {
        if let Expr::Var(name) = &item.expr {
            return Ok(row
                .get(name)
                .cloned()
                .unwrap_or(Binding::Value(CypherValue::Null)));
        }
        Ok(Binding::Value(self.eval_expr(row, &item.expr)?))
    }

    fn project_aggregated(
        &self,
        items: &[ReturnItem],
        columns: &[String],
        rows: &[Row<G::NodeId>],
    ) -> Result<Vec<Projected<G::NodeId>>, CypherError> {
        // Group rows by the values of the non-aggregate (grouping) items.
        let mut group_order: Vec<Vec<CypherValue>> = Vec::new();
        let mut groups: HashMap<Vec<CypherValue>, Vec<usize>> = HashMap::new();

        for (index, row) in rows.iter().enumerate() {
            let mut key = Vec::new();
            for item in items {
                if !item.expr.contains_aggregate() {
                    key.push(self.eval_expr(row, &item.expr)?);
                }
            }
            if !groups.contains_key(&key) {
                group_order.push(key.clone());
            }
            groups.entry(key).or_default().push(index);
        }

        // With no grouping keys and no input rows, aggregates still produce a single row.
        let grouping_keys = items
            .iter()
            .filter(|item| !item.expr.contains_aggregate())
            .count();
        if group_order.is_empty() && grouping_keys == 0 {
            group_order.push(Vec::new());
            groups.insert(Vec::new(), Vec::new());
        }

        let mut output = Vec::with_capacity(group_order.len());
        for key in group_order {
            let row_indices = &groups[&key];
            let group_rows: Vec<&Row<G::NodeId>> =
                row_indices.iter().map(|index| &rows[*index]).collect();
            let representative = group_rows.first().copied();

            let mut bindings = Row::new();
            let mut values = Vec::with_capacity(items.len());
            for (column, item) in columns.iter().zip(items) {
                let binding = if item.expr.contains_aggregate() {
                    Binding::Value(self.eval_aggregate(&item.expr, &group_rows)?)
                } else if let Some(row) = representative {
                    // Preserve the grouping item's binding (e.g. node identity).
                    self.project_binding(row, item)?
                } else {
                    Binding::Value(CypherValue::Null)
                };
                values.push(self.binding_value(&binding));
                bindings.insert(column.clone(), binding);
            }
            output.push(Projected { bindings, values });
        }

        Ok(output)
    }

    fn eval_aggregate(
        &self,
        expr: &Expr,
        group: &[&Row<G::NodeId>],
    ) -> Result<CypherValue, CypherError> {
        let Expr::Aggregate {
            func,
            arg,
            distinct,
        } = expr
        else {
            return Err(CypherError::execution("expected an aggregate function"));
        };

        // count(*) does not evaluate an argument.
        if *func == AggFn::Count && arg.is_none() {
            return Ok(CypherValue::Int(
                i64::try_from(group.len()).unwrap_or(i64::MAX),
            ));
        }

        let arg_expr = arg
            .as_ref()
            .ok_or_else(|| CypherError::execution("aggregate function requires an argument"))?;

        let mut values = Vec::new();
        for row in group {
            let value = self.eval_expr(row, arg_expr)?;
            if value != CypherValue::Null {
                values.push(value);
            }
        }

        if *distinct {
            values = dedupe_values(values);
        }

        Ok(match func {
            AggFn::Count => CypherValue::Int(i64::try_from(values.len()).unwrap_or(i64::MAX)),
            AggFn::Collect => CypherValue::List(values),
            AggFn::Min => values
                .into_iter()
                .min_by(CypherValue::total_cmp)
                .unwrap_or(CypherValue::Null),
            AggFn::Max => values
                .into_iter()
                .max_by(CypherValue::total_cmp)
                .unwrap_or(CypherValue::Null),
            AggFn::Sum => CypherValue::Int(values.iter().filter_map(CypherValue::as_int).sum()),
            AggFn::Avg => {
                let numbers: Vec<i64> = values.iter().filter_map(CypherValue::as_int).collect();
                if numbers.is_empty() {
                    CypherValue::Null
                } else {
                    CypherValue::Int(
                        numbers.iter().sum::<i64>() / i64::try_from(numbers.len()).unwrap_or(1),
                    )
                }
            }
        })
    }
}

/// Applies DISTINCT, ORDER BY, SKIP, and LIMIT to a set of projected rows.
fn finalize<N>(
    projection: &Projection,
    columns: &[String],
    mut projected: Vec<Projected<N>>,
) -> Result<Vec<Projected<N>>, CypherError> {
    if projection.distinct {
        dedupe_projected(&mut projected);
    }

    if !projection.order_by.is_empty() {
        let mut keys: Vec<usize> = Vec::with_capacity(projection.order_by.len());
        for item in &projection.order_by {
            keys.push(resolve_order_column(item, &projection.items, columns)?);
        }

        projected.sort_by(|a, b| {
            for (key_index, order_item) in keys.iter().zip(&projection.order_by) {
                let ordering = a.values[*key_index].total_cmp(&b.values[*key_index]);
                let ordering = if order_item.descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    if let Some(skip) = projection.skip {
        if skip >= projected.len() {
            projected.clear();
        } else {
            projected.drain(0..skip);
        }
    }

    if let Some(limit) = projection.limit
        && projected.len() > limit
    {
        projected.truncate(limit);
    }

    Ok(projected)
}

fn resolve_order_column(
    order_item: &OrderItem,
    items: &[ReturnItem],
    columns: &[String],
) -> Result<usize, CypherError> {
    // Match by identical projection expression first.
    if let Some(index) = items.iter().position(|item| item.expr == order_item.expr) {
        return Ok(index);
    }

    // Otherwise, a bare variable in ORDER BY may name a projected column or alias.
    if let Expr::Var(name) = &order_item.expr
        && let Some(index) = columns.iter().position(|column| column == name)
    {
        return Ok(index);
    }

    Err(CypherError::execution(format!(
        "ORDER BY expression `{}` must also appear in the projection",
        order_item.expr.display_name()
    )))
}

/// Builds the pushdown map: single-variable, aggregate/EXISTS-free `WHERE` conjuncts keyed by the
/// variable they reference.
fn build_pushdown(where_clause: &Option<Expr>) -> HashMap<String, Vec<Expr>> {
    let mut map: HashMap<String, Vec<Expr>> = HashMap::new();
    if let Some(predicate) = where_clause {
        let mut conjuncts = Vec::new();
        collect_conjuncts(predicate, &mut conjuncts);
        for conjunct in conjuncts {
            if let Some(var) = single_var(conjunct) {
                map.entry(var).or_default().push(conjunct.clone());
            }
        }
    }
    map
}

/// Flattens a conjunction (`a AND b AND ...`) into its individual conjuncts.
fn collect_conjuncts<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::And(a, b) = expr {
        collect_conjuncts(a, out);
        collect_conjuncts(b, out);
    } else {
        out.push(expr);
    }
}

/// Returns the sole variable an expression references if it is pushable (references exactly one
/// variable and contains no aggregate or EXISTS subquery); otherwise `None`.
fn single_var(expr: &Expr) -> Option<String> {
    let mut vars = HashSet::new();
    if !collect_vars(expr, &mut vars) {
        return None;
    }
    if vars.len() == 1 {
        vars.into_iter().next()
    } else {
        None
    }
}

/// Collects the variables referenced by an expression. Returns `false` if the expression contains
/// an aggregate or EXISTS subquery (not eligible for pushdown).
fn collect_vars(expr: &Expr, out: &mut HashSet<String>) -> bool {
    match expr {
        Expr::Var(name) => {
            out.insert(name.clone());
            true
        }
        Expr::Property(var, _) => {
            out.insert(var.clone());
            true
        }
        Expr::Literal(_) => true,
        Expr::List(items) => items.iter().all(|item| collect_vars(item, out)),
        Expr::Not(inner) | Expr::IsNull(inner, _) => collect_vars(inner, out),
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
            let left = collect_vars(a, out);
            let right = collect_vars(b, out);
            left && right
        }
        Expr::Function { args, .. } => args.iter().all(|arg| collect_vars(arg, out)),
        Expr::MapProjection { var, entries } => {
            out.insert(var.clone());
            entries.iter().all(|entry| match entry {
                crate::ast::MapEntry::Property(_) => true,
                crate::ast::MapEntry::Literal(_, expr) => collect_vars(expr, out),
            })
        }
        Expr::Case {
            operand,
            branches,
            default,
        } => {
            let mut ok = operand.as_ref().is_none_or(|o| collect_vars(o, out));
            for (when, then) in branches {
                ok = collect_vars(when, out) && ok;
                ok = collect_vars(then, out) && ok;
            }
            ok = default.as_ref().is_none_or(|d| collect_vars(d, out)) && ok;
            ok
        }
        Expr::Aggregate { .. } | Expr::Exists { .. } => false,
    }
}

/// Whether an expression references only variables in `allowed` (and no aggregates), so it is
/// constant across a pass in which those variables are constant.
fn references_only(expr: &Expr, allowed: &HashSet<String>) -> bool {
    match expr {
        Expr::Var(name) => allowed.contains(name),
        Expr::Property(var, _) => allowed.contains(var),
        Expr::Literal(_) => true,
        Expr::List(items) => items.iter().all(|item| references_only(item, allowed)),
        Expr::Not(inner) | Expr::IsNull(inner, _) => references_only(inner, allowed),
        Expr::And(a, b) | Expr::Or(a, b) | Expr::Compare(a, _, b) => {
            references_only(a, allowed) && references_only(b, allowed)
        }
        Expr::Function { args, .. } => args.iter().all(|arg| references_only(arg, allowed)),
        Expr::MapProjection { var, entries } => {
            allowed.contains(var)
                && entries.iter().all(|entry| match entry {
                    crate::ast::MapEntry::Property(_) => true,
                    crate::ast::MapEntry::Literal(_, expr) => references_only(expr, allowed),
                })
        }
        Expr::Case {
            operand,
            branches,
            default,
        } => {
            operand.as_ref().is_none_or(|o| references_only(o, allowed))
                && branches
                    .iter()
                    .all(|(w, t)| references_only(w, allowed) && references_only(t, allowed))
                && default.as_ref().is_none_or(|d| references_only(d, allowed))
        }
        // Aggregates and existential subqueries are not treated as loop-invariant.
        Expr::Aggregate { .. } | Expr::Exists { .. } => false,
    }
}

/// Chooses a traversal plan for a linear path: if the written start endpoint is not yet bound but
/// the end endpoint is, return a reversed copy so matching starts from the bound node. Otherwise
/// the pattern is returned unchanged. Reversal preserves the set of variable bindings exactly.
fn plan_pattern<N>(pattern: &PathPattern, sample: Option<&Row<N>>) -> PathPattern {
    if should_reverse(pattern, sample) {
        reverse_path(pattern)
    } else {
        pattern.clone()
    }
}

fn should_reverse<N>(pattern: &PathPattern, sample: Option<&Row<N>>) -> bool {
    let Some((_, end)) = pattern.rest.last() else {
        return false; // single node: nothing to reverse
    };
    let start_bound = is_node_bound(&pattern.start.var, sample);
    let end_bound = is_node_bound(&end.var, sample);
    !start_bound && end_bound
}

fn is_node_bound<N>(var: &Option<String>, sample: Option<&Row<N>>) -> bool {
    match (var, sample) {
        (Some(name), Some(row)) => matches!(row.get(name), Some(Binding::Node(_))),
        _ => false,
    }
}

/// Reverses a linear path: walks the node/relationship chain end-to-start and flips each
/// relationship's direction. The resulting pattern matches the same variable bindings.
fn reverse_path(pattern: &PathPattern) -> PathPattern {
    if pattern.rest.is_empty() {
        return pattern.clone();
    }

    let mut nodes: Vec<&NodePattern> = Vec::with_capacity(pattern.rest.len() + 1);
    nodes.push(&pattern.start);
    for (_, node) in &pattern.rest {
        nodes.push(node);
    }
    let rels: Vec<&crate::ast::RelPattern> = pattern.rest.iter().map(|(rel, _)| rel).collect();

    let start = nodes[nodes.len() - 1].clone();
    let mut rest = Vec::with_capacity(rels.len());
    for i in (0..rels.len()).rev() {
        rest.push((reverse_rel(rels[i]), nodes[i].clone()));
    }
    PathPattern { start, rest }
}

fn reverse_rel(rel: &crate::ast::RelPattern) -> crate::ast::RelPattern {
    let mut reversed = rel.clone();
    reversed.direction = match rel.direction {
        Direction::Outgoing => Direction::Incoming,
        Direction::Incoming => Direction::Outgoing,
        Direction::Both => Direction::Both,
    };
    reversed
}

/// Adds the node variables introduced by a pattern to the scope, in declaration order.
fn add_pattern_vars(scope: &mut Vec<String>, pattern: &PathPattern) {
    let mut push = |var: &Option<String>| {
        if let Some(name) = var
            && !scope.contains(name)
        {
            scope.push(name.clone());
        }
    };
    push(&pattern.start.var);
    for (_, node) in &pattern.rest {
        push(&node.var);
    }
}

/// Returns the single argument of a scalar function, or an error if the arity is not exactly one.
fn single_arg<'a>(name: &str, args: &'a [Expr]) -> Result<&'a Expr, CypherError> {
    match args {
        [arg] => Ok(arg),
        _ => Err(CypherError::execution(format!(
            "`{name}` expects exactly one argument"
        ))),
    }
}

fn literal_to_value(literal: &Literal) -> CypherValue {
    match literal {
        Literal::Str(value) => CypherValue::Str(value.clone()),
        Literal::Int(value) => CypherValue::Int(*value),
        Literal::Bool(value) => CypherValue::Bool(*value),
        Literal::Null => CypherValue::Null,
    }
}

fn compare_values(left: &CypherValue, op: CmpOp, right: &CypherValue) -> bool {
    if matches!(left, CypherValue::Null) || matches!(right, CypherValue::Null) {
        return false;
    }

    match op {
        CmpOp::Eq => values_equal(left, right),
        CmpOp::Neq => !values_equal(left, right),
        CmpOp::Lt | CmpOp::Lte | CmpOp::Gt | CmpOp::Gte => {
            if !same_type(left, right) {
                return false;
            }
            let ordering = left.total_cmp(right);
            match op {
                CmpOp::Lt => ordering.is_lt(),
                CmpOp::Lte => ordering.is_le(),
                CmpOp::Gt => ordering.is_gt(),
                CmpOp::Gte => ordering.is_ge(),
                _ => unreachable!(),
            }
        }
        CmpOp::Contains => string_op(left, right, |haystack, needle| haystack.contains(needle)),
        CmpOp::StartsWith => {
            string_op(left, right, |haystack, needle| haystack.starts_with(needle))
        }
        CmpOp::EndsWith => string_op(left, right, |haystack, needle| haystack.ends_with(needle)),
        CmpOp::In => match right {
            CypherValue::List(items) => items.iter().any(|item| values_equal(left, item)),
            _ => false,
        },
    }
}

fn values_equal(left: &CypherValue, right: &CypherValue) -> bool {
    same_type(left, right) && left == right
}

fn same_type(left: &CypherValue, right: &CypherValue) -> bool {
    matches!(
        (left, right),
        (CypherValue::Bool(_), CypherValue::Bool(_))
            | (CypherValue::Int(_), CypherValue::Int(_))
            | (CypherValue::Str(_), CypherValue::Str(_))
            | (CypherValue::Node { .. }, CypherValue::Node { .. })
            | (CypherValue::List(_), CypherValue::List(_))
            | (CypherValue::Map(_), CypherValue::Map(_))
    )
}

fn string_op(left: &CypherValue, right: &CypherValue, op: impl Fn(&str, &str) -> bool) -> bool {
    match (left.as_str(), right.as_str()) {
        (Some(haystack), Some(needle)) => op(haystack, needle),
        _ => false,
    }
}

fn dedupe_projected<N>(output: &mut Vec<Projected<N>>) {
    // Order-preserving dedup by projected values. `CypherValue` is `Hash + Eq`, so a `HashSet`
    // gives O(n) dedup (the old linear scan was O(n^2)).
    let mut seen: HashSet<Vec<CypherValue>> = HashSet::new();
    output.retain(|projected| seen.insert(projected.values.clone()));
}

fn dedupe_values(values: Vec<CypherValue>) -> Vec<CypherValue> {
    // Order-preserving dedup backed by a `HashSet` (was an O(n^2) `contains` scan).
    let mut seen: HashSet<CypherValue> = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(value.clone()))
        .collect()
}
