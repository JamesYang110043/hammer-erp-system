use std::str::FromStr;

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::key::InvalidKey;

use crate::*;

// ---------------------------------------------------------------------------
// AST 建構小工具（span 不參與比較，一律用預設值）
// ---------------------------------------------------------------------------

fn e(kind: ExprKind) -> Expr {
    Expr::new(kind, Span::default())
}

fn num(s: &str) -> Expr {
    e(ExprKind::Number(Decimal::from_str(s).unwrap()))
}

fn string(s: &str) -> Expr {
    e(ExprKind::String(s.to_owned()))
}

fn field(k: &str) -> Expr {
    e(ExprKind::Field(k.to_owned()))
}

fn rel(r: &str, f: &str) -> Expr {
    e(ExprKind::RelationField {
        relation: r.to_owned(),
        field: f.to_owned(),
    })
}

fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
    e(ExprKind::Binary {
        op,
        left: Box::new(l),
        right: Box::new(r),
    })
}

fn un(op: UnaryOp, x: Expr) -> Expr {
    e(ExprKind::Unary {
        op,
        operand: Box::new(x),
    })
}

fn call(func: Function, args: Vec<Expr>) -> Expr {
    e(ExprKind::Call { func, args })
}

fn agg(func: AggregateFunc, d: &str, f: &str) -> Expr {
    e(ExprKind::Aggregate {
        func,
        detail: d.to_owned(),
        field: f.to_owned(),
    })
}

fn err(src: &str) -> ParseError {
    parse(src).expect_err(src)
}

/// 斷言錯誤種類與位置（字元位移 start..end）。
fn assert_err(src: &str, kind: ParseErrorKind, start: usize, end: usize) {
    let error = err(src);
    assert_eq!(error.kind, kind, "公式：{src}");
    assert_eq!(error.span, Span::new(start, end), "公式：{src}");
    assert!(!error.message().is_empty());
}

// ---------------------------------------------------------------------------
// §7.6 語法範例
// ---------------------------------------------------------------------------

#[test]
fn spec_example_field_product() {
    assert_eq!(
        parse("{qty} * {unit_price}").unwrap(),
        bin(BinaryOp::Mul, field("qty"), field("unit_price"))
    );
}

#[test]
fn spec_example_round() {
    assert_eq!(
        parse("ROUND({subtotal} * 0.05, 0)").unwrap(),
        call(
            Function::Round,
            vec![bin(BinaryOp::Mul, field("subtotal"), num("0.05")), num("0")]
        )
    );
}

#[test]
fn spec_example_aggregate() {
    assert_eq!(
        parse("SUM(lines.amount) + {shipping_fee}").unwrap(),
        bin(
            BinaryOp::Add,
            agg(AggregateFunc::Sum, "lines", "amount"),
            field("shipping_fee")
        )
    );
}

#[test]
fn spec_example_relation() {
    assert_eq!(
        parse("{total} * (1 - {customer.discount_rate})").unwrap(),
        bin(
            BinaryOp::Mul,
            field("total"),
            bin(BinaryOp::Sub, num("1"), rel("customer", "discount_rate"))
        )
    );
}

#[test]
fn spec_example_if() {
    assert_eq!(
        parse(r#"IF({status} = "open", DAYS(TODAY(), {due_date}), 0)"#).unwrap(),
        call(
            Function::If,
            vec![
                bin(BinaryOp::Eq, field("status"), string("open")),
                call(
                    Function::Days,
                    vec![call(Function::Today, vec![]), field("due_date")]
                ),
                num("0"),
            ]
        )
    );
}

// ---------------------------------------------------------------------------
// 常數、函式、大小寫
// ---------------------------------------------------------------------------

#[test]
fn number_keeps_decimal_precision() {
    // 0.1 + 0.2 若經過浮點數會失真；這裡必須是精確的十進位值
    let ExprKind::Number(n) = parse("0.1000").unwrap().kind else {
        panic!("應為數字");
    };
    assert_eq!(n.to_string(), "0.1000");
    let ExprKind::Number(n) = parse("12345678901234567890.123").unwrap().kind else {
        panic!("應為數字");
    };
    assert_eq!(n.to_string(), "12345678901234567890.123");
}

#[test]
fn string_escapes() {
    assert_eq!(parse(r#""a\"b\\c""#).unwrap(), string(r#"a"b\c"#));
    assert_eq!(parse(r#""中文""#).unwrap(), string("中文"));
}

#[test]
fn booleans_and_keywords_are_case_insensitive() {
    assert_eq!(
        parse("true or False").unwrap(),
        bin(
            BinaryOp::Or,
            e(ExprKind::Bool(true)),
            e(ExprKind::Bool(false))
        )
    );
    assert_eq!(
        parse("not {a} AnD {b}").unwrap(),
        bin(BinaryOp::And, un(UnaryOp::Not, field("a")), field("b"))
    );
}

#[test]
fn function_names_are_case_insensitive() {
    assert_eq!(
        parse("round(abs({x}), 2)").unwrap(),
        call(
            Function::Round,
            vec![call(Function::Abs, vec![field("x")]), num("2")]
        )
    );
    assert_eq!(
        parse("count(lines.qty)").unwrap(),
        agg(AggregateFunc::Count, "lines", "qty")
    );
}

#[test]
fn all_aggregates_and_variadic_functions() {
    for (name, func) in [
        ("SUM", AggregateFunc::Sum),
        ("AVG", AggregateFunc::Avg),
        ("COUNT", AggregateFunc::Count),
        ("MIN", AggregateFunc::Min),
        ("MAX", AggregateFunc::Max),
    ] {
        assert_eq!(
            parse(&format!("{name}(lines.amount)")).unwrap(),
            agg(func, "lines", "amount")
        );
    }
    assert_eq!(
        parse(r#"CONCAT({a}, "-", {b})"#).unwrap(),
        call(Function::Concat, vec![field("a"), string("-"), field("b")])
    );
    assert_eq!(
        parse("COALESCE({a})").unwrap(),
        call(Function::Coalesce, vec![field("a")])
    );
}

// ---------------------------------------------------------------------------
// 優先順序與結合性
// ---------------------------------------------------------------------------

#[test]
fn precedence_mul_over_add() {
    assert_eq!(
        parse("1 + 2 * 3").unwrap(),
        bin(
            BinaryOp::Add,
            num("1"),
            bin(BinaryOp::Mul, num("2"), num("3"))
        )
    );
}

#[test]
fn precedence_not_below_comparison() {
    assert_eq!(
        parse("NOT {a} = {b}").unwrap(),
        un(UnaryOp::Not, bin(BinaryOp::Eq, field("a"), field("b")))
    );
}

#[test]
fn precedence_concat_above_comparison() {
    assert_eq!(
        parse("{a} & {b} = {c}").unwrap(),
        bin(
            BinaryOp::Eq,
            bin(BinaryOp::Concat, field("a"), field("b")),
            field("c")
        )
    );
}

#[test]
fn precedence_add_above_concat() {
    assert_eq!(
        parse("{a} & 1 + 2").unwrap(),
        bin(
            BinaryOp::Concat,
            field("a"),
            bin(BinaryOp::Add, num("1"), num("2"))
        )
    );
}

#[test]
fn precedence_and_above_or() {
    assert_eq!(
        parse("{a} OR {b} AND {c}").unwrap(),
        bin(
            BinaryOp::Or,
            field("a"),
            bin(BinaryOp::And, field("b"), field("c"))
        )
    );
}

#[test]
fn unary_minus_binds_tightest() {
    assert_eq!(
        parse("-2 * 3").unwrap(),
        bin(BinaryOp::Mul, un(UnaryOp::Neg, num("2")), num("3"))
    );
    assert_eq!(
        parse("1 - -2").unwrap(),
        bin(BinaryOp::Sub, num("1"), un(UnaryOp::Neg, num("2")))
    );
}

#[test]
fn binary_operators_are_left_associative() {
    assert_eq!(
        parse("1 - 2 - 3").unwrap(),
        bin(
            BinaryOp::Sub,
            bin(BinaryOp::Sub, num("1"), num("2")),
            num("3")
        )
    );
    assert_eq!(
        parse("8 / 4 / 2").unwrap(),
        bin(
            BinaryOp::Div,
            bin(BinaryOp::Div, num("8"), num("4")),
            num("2")
        )
    );
}

#[test]
fn parentheses_override_precedence() {
    assert_eq!(
        parse("(1 + 2) * 3").unwrap(),
        bin(
            BinaryOp::Mul,
            bin(BinaryOp::Add, num("1"), num("2")),
            num("3")
        )
    );
}

#[test]
fn all_comparison_operators() {
    for (src, op) in [
        ("=", BinaryOp::Eq),
        ("<>", BinaryOp::Ne),
        ("<", BinaryOp::Lt),
        ("<=", BinaryOp::Le),
        (">", BinaryOp::Gt),
        (">=", BinaryOp::Ge),
    ] {
        assert_eq!(
            parse(&format!("{{a}} {src} {{b}}")).unwrap(),
            bin(op, field("a"), field("b"))
        );
    }
}

// ---------------------------------------------------------------------------
// 語法錯誤：種類與位置（字元位移）
// ---------------------------------------------------------------------------

#[test]
fn error_empty() {
    assert_err("", ParseErrorKind::Empty, 0, 0);
    assert_err("   ", ParseErrorKind::Empty, 0, 3);
}

#[test]
fn error_unclosed_paren() {
    // 指向沒有閉合的 (
    assert_err("(1 + 2", ParseErrorKind::UnclosedParen, 0, 1);
    assert_err("ROUND({a}, 2", ParseErrorKind::UnclosedParen, 5, 6);
    assert_err("SUM(lines.amount", ParseErrorKind::UnclosedParen, 3, 4);
}

#[test]
fn error_unclosed_string() {
    assert_err(r#"{a} & "abc"#, ParseErrorKind::UnclosedString, 6, 10);
    // 結尾的 \" 是跳脫，不算閉合
    assert_err(r#""abc\""#, ParseErrorKind::UnclosedString, 0, 6);
}

#[test]
fn error_invalid_escape() {
    assert_err(r#""a\nb""#, ParseErrorKind::InvalidEscape(Some('n')), 2, 4);
}

#[test]
fn error_unknown_function() {
    assert_err(
        "1 + FOO({a})",
        ParseErrorKind::UnknownFunction("FOO".to_owned()),
        4,
        7,
    );
}

#[test]
fn error_wrong_arg_count() {
    assert_err(
        "ROUND({a})",
        ParseErrorKind::WrongArgCount {
            func: "ROUND",
            expected: Arity::Exact(2),
            found: 1,
        },
        0,
        10,
    );
    assert_err(
        "1 + IF({a}, 1)",
        ParseErrorKind::WrongArgCount {
            func: "IF",
            expected: Arity::Exact(3),
            found: 2,
        },
        4,
        14,
    );
    assert_err(
        "TODAY(1)",
        ParseErrorKind::WrongArgCount {
            func: "TODAY",
            expected: Arity::Exact(0),
            found: 1,
        },
        0,
        8,
    );
    assert_err(
        "CONCAT()",
        ParseErrorKind::WrongArgCount {
            func: "CONCAT",
            expected: Arity::AtLeast(1),
            found: 0,
        },
        0,
        8,
    );
}

#[test]
fn error_nested_relation() {
    assert_err("1 + {a.b.c}", ParseErrorKind::NestedRelation, 4, 11);
}

#[test]
fn error_unclosed_field_ref() {
    assert_err("1 + {qty", ParseErrorKind::UnclosedFieldRef, 4, 5);
}

#[test]
fn error_invalid_field_key() {
    assert_err(
        "{Qty}",
        ParseErrorKind::InvalidKey {
            key: "Qty".to_owned(),
            reason: InvalidKey::InvalidStart,
        },
        0,
        5,
    );
    assert_err(
        "{}",
        ParseErrorKind::InvalidKey {
            key: String::new(),
            reason: InvalidKey::Empty,
        },
        0,
        2,
    );
}

#[test]
fn error_invalid_aggregate_arg() {
    assert_err(
        "SUM({amount})",
        ParseErrorKind::InvalidAggregateArg { func: "SUM" },
        4,
        12,
    );
    assert_err(
        "SUM(lines.amount, 1)",
        ParseErrorKind::InvalidAggregateArg { func: "SUM" },
        4,
        17,
    );
    assert_err(
        "SUM(lines . amount)",
        ParseErrorKind::InvalidAggregateArg { func: "SUM" },
        4,
        18,
    );
}

#[test]
fn error_bare_identifier() {
    assert_err(
        "qty * 2",
        ParseErrorKind::BareIdentifier("qty".to_owned()),
        0,
        3,
    );
}

#[test]
fn error_unexpected_token_and_end() {
    assert_err(
        "1 + * 2",
        ParseErrorKind::UnexpectedToken {
            found: "*".to_owned(),
        },
        4,
        5,
    );
    assert_err("1 +", ParseErrorKind::UnexpectedEnd, 3, 3);
    assert_err(
        "(1))",
        ParseErrorKind::UnexpectedToken {
            found: ")".to_owned(),
        },
        3,
        4,
    );
    assert_err(
        "1 2",
        ParseErrorKind::UnexpectedToken {
            found: "2".to_owned(),
        },
        2,
        3,
    );
}

#[test]
fn error_unexpected_char_uses_char_offset() {
    // 前面的中文字串佔多個 byte，但位置以字元計
    assert_err(r#""單價" & #"#, ParseErrorKind::UnexpectedChar('#'), 7, 8);
}

#[test]
fn error_message_is_traditional_chinese() {
    let error = err("ROUND({a})");
    assert_eq!(error.message(), "函式 ROUND 需要 2 個參數，但提供了 1 個");
    assert_eq!(
        error.to_string(),
        "第 1 個字元：函式 ROUND 需要 2 個參數，但提供了 1 個"
    );
}

// ---------------------------------------------------------------------------
// 長度與深度上限
// ---------------------------------------------------------------------------

#[test]
fn error_too_long() {
    let src = "1".repeat(MAX_FORMULA_LEN + 1);
    assert_err(
        &src,
        ParseErrorKind::TooLong {
            max: MAX_FORMULA_LEN,
        },
        MAX_FORMULA_LEN,
        MAX_FORMULA_LEN + 1,
    );
    // 剛好上限可以解析
    let src = format!("1{}", " ".repeat(MAX_FORMULA_LEN - 1));
    assert!(parse(&src).is_ok());
}

#[test]
fn error_too_deep_parentheses() {
    let src = format!(
        "{}1{}",
        "(".repeat(MAX_DEPTH + 1),
        ")".repeat(MAX_DEPTH + 1)
    );
    assert_err(
        &src,
        ParseErrorKind::TooDeep { max: MAX_DEPTH },
        MAX_DEPTH,
        MAX_DEPTH + 1,
    );
    let src = format!("{}1{}", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
    assert!(parse(&src).is_ok());
}

#[test]
fn deep_nesting_within_length_limit_does_not_overflow() {
    // 在長度上限內盡量堆疊各種巢狀，都必須回傳錯誤而不是 stack overflow
    for unit in ["(", "-", "NOT ", "ABS(", "IF(1,1,"] {
        let n = MAX_FORMULA_LEN / unit.len();
        let src = unit.repeat(n);
        let error = err(&src);
        assert_eq!(
            error.kind,
            ParseErrorKind::TooDeep { max: MAX_DEPTH },
            "{unit}"
        );
    }
}

#[test]
fn long_flat_chain_parses_and_round_trips() {
    // 左結合的長鏈不受深度上限影響，但不可在解析、輸出時溢位
    let src = vec!["1"; MAX_FORMULA_LEN / 4].join(" + ");
    assert!(src.len() <= MAX_FORMULA_LEN);
    let ast = parse(&src).unwrap();
    assert_eq!(parse(&ast.to_string()).unwrap(), ast);
}

// ---------------------------------------------------------------------------
// 正規化文字與 round trip
// ---------------------------------------------------------------------------

#[test]
fn normalized_text() {
    for (src, expected) in [
        ("{qty}*{unit_price}", "{qty} * {unit_price}"),
        ("round( {subtotal}*0.05 ,0 )", "ROUND({subtotal} * 0.05, 0)"),
        (
            "sum(lines.amount)+{shipping_fee}",
            "SUM(lines.amount) + {shipping_fee}",
        ),
        ("((1 + 2)) * 3", "(1 + 2) * 3"),
        ("1 + (2 * 3)", "1 + 2 * 3"),
        ("1 - (2 - 3)", "1 - (2 - 3)"),
        ("(1 - 2) - 3", "1 - 2 - 3"),
        ("not ({a} or {b})", "NOT ({a} OR {b})"),
        ("-(1 + 2)", "-(1 + 2)"),
        ("- - 1", "--1"),
        ("true", "TRUE"),
        (r#""a\"b\\""#, r#""a\"b\\""#),
    ] {
        assert_eq!(parse(src).unwrap().to_string(), expected, "公式：{src}");
    }
}

#[test]
fn round_trip() {
    for src in [
        "{qty} * {unit_price}",
        "ROUND({subtotal} * 0.05, 0)",
        "SUM(lines.amount) + {shipping_fee}",
        "{total} * (1 - {customer.discount_rate})",
        r#"IF({status} = "open", DAYS(TODAY(), {due_date}), 0)"#,
        "1 + 2 * 3",
        "(1 + 2) * 3",
        "1 - (2 - 3)",
        "8 / (4 / 2)",
        "NOT {a} = {b}",
        "NOT ({a} AND {b}) OR {c}",
        "{a} & {b} = {c}",
        "{a} & ({b} = {c})",
        "({a} OR {b}) AND {c}",
        "-{a} * -(1 + {b})",
        "NOT NOT TRUE",
        r#"CONCAT("say \"hi\"", "\\", {name})"#,
        "COALESCE({a}, {b}, 0) >= 0.0010",
        "AVG(lines.price) <> MAX(lines.price)",
        "MIN(lines.qty) < COUNT(lines.qty)",
        "ABS({a} - {b}) <= 1",
        "{a} > 1 AND {b} < 2 OR {c} = 3",
    ] {
        let ast = parse(src).unwrap();
        let text = ast.to_string();
        assert_eq!(parse(&text).unwrap(), ast, "公式：{src} → {text}");
    }
}

#[test]
fn display_adds_parentheses_for_hand_built_ast() {
    // 解析器不會產生的結構（比較運算的右邊是 NOT），輸出時也要加括號讓它能再解析回來
    let ast = bin(BinaryOp::Eq, field("a"), un(UnaryOp::Not, field("b")));
    assert_eq!(ast.to_string(), "{a} = (NOT {b})");
    assert_eq!(parse(&ast.to_string()).unwrap(), ast);
}

#[test]
fn spans_point_into_source() {
    let ast = parse("1 + ROUND({a}, 2)").unwrap();
    assert_eq!(ast.span, Span::new(0, 17));
    let ExprKind::Binary { right, .. } = ast.kind else {
        panic!("應為二元運算");
    };
    assert_eq!(right.span, Span::new(4, 17));
}
