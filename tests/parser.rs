use cypher_parser::ast::{AggFn, CmpOp, Direction, Expr, Literal};
use cypher_parser::parse;

#[test]
fn parses_basic_match_return() {
    let query = parse("MATCH (c:Class) RETURN c.name").unwrap();
    assert_eq!(query.patterns.len(), 1);
    let start = &query.patterns[0].start;
    assert_eq!(start.var.as_deref(), Some("c"));
    assert_eq!(start.labels, vec!["Class".to_string()]);
    assert!(query.patterns[0].rest.is_empty());
    assert_eq!(query.return_clause.items.len(), 1);
    assert_eq!(
        query.return_clause.items[0].expr,
        Expr::Property("c".into(), "name".into())
    );
}

#[test]
fn parses_label_disjunction() {
    let query = parse("MATCH (n:Class|Module) RETURN n").unwrap();
    assert_eq!(
        query.patterns[0].start.labels,
        vec!["Class".to_string(), "Module".to_string()]
    );
}

#[test]
fn parses_inline_properties() {
    let query = parse("MATCH (c:Class {name: 'Foo'}) RETURN c").unwrap();
    let props = &query.patterns[0].start.props;
    assert_eq!(props.len(), 1);
    assert_eq!(props[0].0, "name");
    assert_eq!(props[0].1, Literal::Str("Foo".into()));
}

#[test]
fn parses_relationship_directions() {
    let outgoing = parse("MATCH (a)-[:INHERITS]->(b) RETURN a").unwrap();
    assert_eq!(
        outgoing.patterns[0].rest[0].0.direction,
        Direction::Outgoing
    );
    assert_eq!(
        outgoing.patterns[0].rest[0].0.types,
        vec!["INHERITS".to_string()]
    );

    let incoming = parse("MATCH (a)<-[:INHERITS]-(b) RETURN a").unwrap();
    assert_eq!(
        incoming.patterns[0].rest[0].0.direction,
        Direction::Incoming
    );

    let both = parse("MATCH (a)-[:INHERITS]-(b) RETURN a").unwrap();
    assert_eq!(both.patterns[0].rest[0].0.direction, Direction::Both);
}

#[test]
fn parses_variable_length() {
    let query = parse("MATCH (a)-[:INHERITS*2..5]->(b) RETURN a").unwrap();
    let length = query.patterns[0].rest[0].0.length.unwrap();
    assert_eq!(length.min, 2);
    assert_eq!(length.max, Some(5));

    let unbounded = parse("MATCH (a)-[:OWNS*]->(b) RETURN a").unwrap();
    let length = unbounded.patterns[0].rest[0].0.length.unwrap();
    assert_eq!(length.min, 1);
    assert_eq!(length.max, None);

    let exact = parse("MATCH (a)-[:OWNS*3]->(b) RETURN a").unwrap();
    let length = exact.patterns[0].rest[0].0.length.unwrap();
    assert_eq!(length.min, 3);
    assert_eq!(length.max, Some(3));
}

#[test]
fn parses_aggregation_and_alias() {
    let query = parse("MATCH (c:Class) RETURN c.name, count(*) AS total").unwrap();
    assert_eq!(query.return_clause.items[1].alias.as_deref(), Some("total"));
    assert_eq!(
        query.return_clause.items[1].expr,
        Expr::Aggregate {
            func: AggFn::Count,
            arg: None,
            distinct: false,
        }
    );
}

#[test]
fn parses_where_and_order_limit() {
    let query =
        parse("MATCH (c:Class) WHERE c.name CONTAINS 'Service' RETURN c.name ORDER BY c.name DESC LIMIT 5").unwrap();
    let Some(Expr::Compare(_, op, _)) = query.where_clause else {
        panic!("expected comparison");
    };
    assert_eq!(op, CmpOp::Contains);
    assert_eq!(query.order_by.len(), 1);
    assert!(query.order_by[0].descending);
    assert_eq!(query.limit, Some(5));
}

#[test]
fn parses_in_with_list_literal() {
    let query = parse("MATCH (c:Class) WHERE c.name IN ['Dog', 'Cat'] RETURN c").unwrap();
    let Some(Expr::Compare(left, op, right)) = query.where_clause else {
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
    let Some(Expr::Compare(_, _, right)) = empty.where_clause else {
        panic!("expected comparison");
    };
    assert_eq!(*right, Expr::List(vec![]));
}

#[test]
fn parses_is_null() {
    let not_null = parse("MATCH (c) WHERE c.line IS NOT NULL RETURN c").unwrap();
    assert_eq!(
        not_null.where_clause,
        Some(Expr::IsNull(
            Box::new(Expr::Property("c".into(), "line".into())),
            true,
        ))
    );

    let is_null = parse("MATCH (c) WHERE c.missing IS NULL RETURN c").unwrap();
    assert_eq!(
        is_null.where_clause,
        Some(Expr::IsNull(
            Box::new(Expr::Property("c".into(), "missing".into())),
            false,
        ))
    );
}

#[test]
fn parses_return_star() {
    let query = parse("MATCH (c)-[:INHERITS]->(p) RETURN *").unwrap();
    assert!(query.return_clause.star);
    assert!(query.return_clause.items.is_empty());

    let distinct = parse("MATCH (c) RETURN DISTINCT *").unwrap();
    assert!(distinct.return_clause.star);
    assert!(distinct.return_clause.distinct);
}

#[test]
fn parses_function_calls() {
    let query =
        parse("MATCH (c) WHERE toLower(c.name) CONTAINS 'service' RETURN size(c.name)").unwrap();
    let Some(Expr::Compare(left, CmpOp::Contains, _)) = query.where_clause else {
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
        query.return_clause.items[0].expr,
        Expr::Function {
            name: "size".into(),
            args: vec![Expr::Property("c".into(), "name".into())],
        }
    );

    let coalesce = parse("MATCH (c) RETURN coalesce(c.nick, c.name, 'unknown')").unwrap();
    let Expr::Function { name, args } = &coalesce.return_clause.items[0].expr else {
        panic!("expected function");
    };
    assert_eq!(name, "coalesce");
    assert_eq!(args.len(), 3);
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
