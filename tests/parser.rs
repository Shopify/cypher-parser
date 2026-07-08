use cypher_parser::ast::{
    AggFn, Clause, CmpOp, Direction, Expr, Literal, MatchClause, PathPattern, Query,
};
use cypher_parser::parse;

fn first_match(query: &Query) -> &MatchClause {
    match query.clauses.first() {
        Some(Clause::Match(m)) => m,
        _ => panic!("expected the first clause to be MATCH"),
    }
}

fn patterns(query: &Query) -> &[PathPattern] {
    &first_match(query).patterns
}

fn where_clause(query: &Query) -> Option<Expr> {
    first_match(query).where_clause.clone()
}

#[test]
fn parses_basic_match_return() {
    let query = parse("MATCH (c:Class) RETURN c.name").unwrap();
    assert_eq!(patterns(&query).len(), 1);
    let start = &patterns(&query)[0].start;
    assert_eq!(start.var.as_deref(), Some("c"));
    assert_eq!(start.labels, vec!["Class".to_string()]);
    assert!(patterns(&query)[0].rest.is_empty());
    assert_eq!(query.result.items.len(), 1);
    assert_eq!(
        query.result.items[0].expr,
        Expr::Property("c".into(), "name".into())
    );
}

#[test]
fn parses_label_disjunction() {
    let query = parse("MATCH (n:Class|Module) RETURN n").unwrap();
    assert_eq!(
        patterns(&query)[0].start.labels,
        vec!["Class".to_string(), "Module".to_string()]
    );
}

#[test]
fn parses_inline_properties() {
    let query = parse("MATCH (c:Class {name: 'Foo'}) RETURN c").unwrap();
    let props = &patterns(&query)[0].start.props;
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].0, "name");
    assert_eq!(props[0].1, Literal::Str("Foo".into()));
}

#[test]
fn parses_relationship_directions() {
    let outgoing = parse("MATCH (a)-[:INHERITS]->(b) RETURN a").unwrap();
    assert_eq!(
        patterns(&outgoing)[0].rest[0].0.direction,
        Direction::Outgoing
    );
    assert_eq!(
        patterns(&outgoing)[0].rest[0].0.types,
        vec!["INHERITS".to_string()]
    );

    let incoming = parse("MATCH (a)<-[:INHERITS]-(b) RETURN a").unwrap();
    assert_eq!(
        patterns(&incoming)[0].rest[0].0.direction,
        Direction::Incoming
    );

    let both = parse("MATCH (a)-[:INHERITS]-(b) RETURN a").unwrap();
    assert_eq!(patterns(&both)[0].rest[0].0.direction, Direction::Both);
}

#[test]
fn parses_variable_length() {
    let query = parse("MATCH (a)-[:INHERITS*2..5]->(b) RETURN a").unwrap();
    let length = patterns(&query)[0].rest[0].0.length.unwrap();
    assert_eq!(length.min, 2);
    assert_eq!(length.max, Some(5));

    let unbounded = parse("MATCH (a)-[:OWNS*]->(b) RETURN a").unwrap();
    let length = patterns(&unbounded)[0].rest[0].0.length.unwrap();
    assert_eq!(length.min, 1);
    assert_eq!(length.max, None);

    let exact = parse("MATCH (a)-[:OWNS*3]->(b) RETURN a").unwrap();
    let length = patterns(&exact)[0].rest[0].0.length.unwrap();
    assert_eq!(length.min, 3);
    assert_eq!(length.max, Some(3));
}

#[test]
fn parses_aggregation_and_alias() {
    let query = parse("MATCH (c:Class) RETURN c.name, count(*) AS total").unwrap();
    assert_eq!(query.result.items[1].alias.as_deref(), Some("total"));
    assert_eq!(
        query.result.items[1].expr,
        Expr::Aggregate {
            func: AggFn::Count,
            arg: None,
            distinct: false,
        }
    );
}

#[test]
fn parses_where_and_order_limit() {
    let query = parse(
        "MATCH (c:Class) WHERE c.name CONTAINS 'Service' RETURN c.name ORDER BY c.name DESC LIMIT 5",
    )
    .unwrap();
    let Some(Expr::Compare(_, op, _)) = where_clause(&query) else {
        panic!("expected comparison");
    };
    assert_eq!(op, CmpOp::Contains);
    assert_eq!(query.result.order_by.len(), 1);
    assert!(query.result.order_by[0].descending);
    assert_eq!(query.result.limit, Some(5));
}

#[test]
fn parses_in_with_list_literal() {
    let query = parse("MATCH (c:Class) WHERE c.name IN ['Dog', 'Cat'] RETURN c").unwrap();
    let Some(Expr::Compare(left, op, right)) = where_clause(&query) else {
        panic!("expected comparison");
    };
    assert_eq!(*left, Expr::Property("c".into(), "name".into()));
    assert_eq!(op, CmpOp::In);
    assert_eq!(
        *right,
        Expr::List(vec![
            Expr::Literal(Literal::Str("Dog".into())),
            Expr::Literal(Literal::Str("Cat".into())),
        ])
    );

    // Empty list literal is valid.
    let empty = parse("MATCH (c) WHERE c.name IN [] RETURN c").unwrap();
    let Some(Expr::Compare(_, _, right)) = where_clause(&empty) else {
        panic!("expected comparison");
    };
    assert_eq!(*right, Expr::List(vec![]));
}

#[test]
fn parses_is_null() {
    let not_null = parse("MATCH (c) WHERE c.line IS NOT NULL RETURN c").unwrap();
    assert_eq!(
        where_clause(&not_null),
        Some(Expr::IsNull(
            Box::new(Expr::Property("c".into(), "line".into())),
            true,
        ))
    );

    let is_null = parse("MATCH (c) WHERE c.missing IS NULL RETURN c").unwrap();
    assert_eq!(
        where_clause(&is_null),
        Some(Expr::IsNull(
            Box::new(Expr::Property("c".into(), "missing".into())),
            false,
        ))
    );
}

#[test]
fn parses_return_star() {
    let query = parse("MATCH (c)-[:INHERITS]->(p) RETURN *").unwrap();
    assert!(query.result.star);
    assert!(query.result.items.is_empty());

    let distinct = parse("MATCH (c) RETURN DISTINCT *").unwrap();
    assert!(distinct.result.star);
    assert!(distinct.result.distinct);
}

#[test]
fn parses_function_calls() {
    let query =
        parse("MATCH (c) WHERE toLower(c.name) CONTAINS 'service' RETURN size(c.name)").unwrap();
    let Some(Expr::Compare(left, CmpOp::Contains, _)) = where_clause(&query) else {
        panic!("expected comparison");
    };
    assert_eq!(
        *left,
        Expr::Function {
            name: "toLower".into(),
            args: vec![Expr::Property("c".into(), "name".into())],
        }
    );
    assert_eq!(
        query.result.items[0].expr,
        Expr::Function {
            name: "size".into(),
            args: vec![Expr::Property("c".into(), "name".into())],
        }
    );

    let coalesce = parse("MATCH (c) RETURN coalesce(c.nick, c.name, 'unknown')").unwrap();
    let Expr::Function { name, args } = &coalesce.result.items[0].expr else {
        panic!("expected function");
    };
    assert_eq!(name, "coalesce");
    assert_eq!(args.len(), 3);
}

#[test]
fn parses_with_clause() {
    let query =
        parse("MATCH (c:Class) WITH c.name AS n, count(*) AS total WHERE total > 1 RETURN n")
            .unwrap();
    assert_eq!(query.clauses.len(), 2);

    let Some(Clause::With(with)) = query.clauses.get(1) else {
        panic!("expected the second clause to be WITH");
    };
    assert_eq!(with.projection.items.len(), 2);
    assert_eq!(with.projection.items[0].alias.as_deref(), Some("n"));
    assert_eq!(with.projection.items[1].alias.as_deref(), Some("total"));
    assert!(with.where_clause.is_some());

    assert_eq!(query.result.items.len(), 1);
    assert_eq!(query.result.items[0].expr, Expr::Var("n".into()));
}

#[test]
fn parses_optional_match() {
    let query = parse("MATCH (a:Class) OPTIONAL MATCH (a)-[:INHERITS]->(b) RETURN a").unwrap();
    assert_eq!(query.clauses.len(), 2);

    let Some(Clause::Match(m0)) = query.clauses.first() else {
        panic!("expected MATCH");
    };
    assert!(!m0.optional);

    let Some(Clause::Match(m1)) = query.clauses.get(1) else {
        panic!("expected OPTIONAL MATCH");
    };
    assert!(m1.optional);
    assert_eq!(m1.patterns[0].rest.len(), 1);
}

#[test]
fn parses_exists_predicate() {
    // NOT EXISTS { ... } parses as Not(Exists { ... }).
    let query = parse("MATCH (c) WHERE NOT EXISTS { (c)-[:INHERITS]->() } RETURN c").unwrap();
    let Some(Expr::Not(inner)) = where_clause(&query) else {
        panic!("expected NOT");
    };
    let Expr::Exists {
        patterns,
        where_clause: inner_where,
    } = *inner
    else {
        panic!("expected EXISTS");
    };
    assert_eq!(patterns.len(), 1);
    assert!(inner_where.is_none());

    // EXISTS with an inner WHERE and an explicit MATCH keyword.
    let with_where = parse(
        "MATCH (c) WHERE EXISTS { MATCH (c)-[:INHERITS]->(p) WHERE p.name = 'Animal' } RETURN c",
    )
    .unwrap();
    let Some(Expr::Exists { where_clause, .. }) = where_clause(&with_where) else {
        panic!("expected EXISTS");
    };
    assert!(where_clause.is_some());
}

#[test]
fn parses_unwind() {
    let query = parse("UNWIND [1, 2, 3] AS x RETURN x").unwrap();
    assert_eq!(query.clauses.len(), 1);
    let Some(Clause::Unwind(u)) = query.clauses.first() else {
        panic!("expected UNWIND");
    };
    assert_eq!(u.var, "x");
    assert!(matches!(u.expr, Expr::List(_)));
}

#[test]
fn parses_case_expressions() {
    let generic = parse("MATCH (c) RETURN CASE WHEN c.name = 'Dog' THEN 1 ELSE 0 END").unwrap();
    let Expr::Case {
        operand,
        branches,
        default,
    } = &generic.result.items[0].expr
    else {
        panic!("expected CASE");
    };
    assert!(operand.is_none());
    assert_eq!(branches.len(), 1);
    assert!(default.is_some());

    let simple = parse("MATCH (c) RETURN CASE c.name WHEN 'Dog' THEN 1 END").unwrap();
    let Expr::Case {
        operand, default, ..
    } = &simple.result.items[0].expr
    else {
        panic!("expected CASE");
    };
    assert!(operand.is_some());
    assert!(default.is_none());
}

#[test]
fn parses_comments() {
    let query =
        parse("MATCH (c:Class) // find classes\n WHERE c.name = 'Dog' /* inline */ RETURN c")
            .unwrap();
    assert_eq!(patterns(&query).len(), 1);
    assert!(where_clause(&query).is_some());
}

#[test]
fn parses_map_projection() {
    let query = parse("MATCH (c) RETURN c { .name, kind: 'x' }").unwrap();
    let Expr::MapProjection { var, entries } = &query.result.items[0].expr else {
        panic!("expected map projection");
    };
    assert_eq!(var, "c");
    assert_eq!(entries.len(), 2);
}

#[test]
fn parses_negative_literals() {
    // In a WHERE comparison.
    let query = parse("MATCH (c) WHERE c.line > -1 RETURN c").unwrap();
    let Some(Expr::Compare(_, _, right)) = where_clause(&query) else {
        panic!("expected comparison");
    };
    assert_eq!(*right, Expr::Literal(Literal::Int(-1)));

    // In an inline node property map.
    let props = parse("MATCH (c {line: -5}) RETURN c").unwrap();
    assert_eq!(patterns(&props)[0].start.props[0].1, Literal::Int(-5));

    // In a list literal.
    let list = parse("UNWIND [-1, -2, 3] AS x RETURN x").unwrap();
    let Some(Clause::Unwind(u)) = list.clauses.first() else {
        panic!("expected UNWIND");
    };
    assert_eq!(
        u.expr,
        Expr::List(vec![
            Expr::Literal(Literal::Int(-1)),
            Expr::Literal(Literal::Int(-2)),
            Expr::Literal(Literal::Int(3)),
        ])
    );
}

#[test]
fn rejects_invalid_syntax() {
    assert!(parse("MATCH (c:Class RETURN c").is_err());
    assert!(parse("RETURN c").is_err());
    assert!(parse("MATCH (c) RETURN").is_err());
    assert!(parse("MATCH (a)<-[:INHERITS]->(b) RETURN a").is_err());
    assert!(parse("MATCH (c) WHERE c.x IS RETURN c").is_err());
    assert!(parse("MATCH (c) WHERE c.x IN ['a' RETURN c").is_err());
}
