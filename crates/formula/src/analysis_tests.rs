//! 相依分析與型別檢查的單元測試。

use std::collections::HashSet;

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::DataType;
use serde_json::json;

use crate::*;

fn p(src: &str) -> Expr {
    parse(src).unwrap()
}

// ---------------------------------------------------------------------------
// 取出相依
// ---------------------------------------------------------------------------

fn dep_field(f: &str) -> Dependency {
    Dependency::Field {
        field: f.to_owned(),
    }
}

fn dep_rel(r: &str, f: &str) -> Dependency {
    Dependency::Relation {
        relation: r.to_owned(),
        field: f.to_owned(),
    }
}

fn dep_detail(d: &str, f: &str) -> Dependency {
    Dependency::Detail {
        detail: d.to_owned(),
        field: f.to_owned(),
    }
}

#[test]
fn deps_local_fields() {
    assert_eq!(
        dependencies(&p("{qty} * {unit_price} + {qty}")),
        vec![dep_field("qty"), dep_field("unit_price")]
    );
}

#[test]
fn deps_relation_and_aggregate() {
    let deps = dependencies(&p(
        "IF(NOT {done}, SUM(lines.amount) * (1 - {customer.discount_rate}), -ABS(COUNT(lines.sku)))",
    ));
    assert_eq!(
        deps,
        vec![
            dep_field("done"),
            dep_rel("customer", "discount_rate"),
            dep_detail("lines", "amount"),
            dep_detail("lines", "sku"),
        ]
    );
}

#[test]
fn deps_constants_only() {
    assert!(dependencies(&p("ROUND(1.5, 0) & \"x\" & TRUE & TODAY()")).is_empty());
}

#[test]
fn deps_ignore_formatting() {
    assert_eq!(
        dependencies(&p("{b}+{a}")),
        dependencies(&p("  {a}  +  {b} "))
    );
}

#[test]
fn deps_json_format() {
    let deps = dependencies(&p("{qty} + {customer.rate} + SUM(lines.amount)"));
    let j = deps_to_json(&deps);
    assert_eq!(
        j,
        json!([
            {"kind": "field", "field": "qty"},
            {"kind": "relation", "relation": "customer", "field": "rate"},
            {"kind": "detail", "detail": "lines", "field": "amount"},
        ])
    );
    // serde 的輸出與手寫的 JSON 相同，可以讀回
    assert_eq!(serde_json::to_value(&deps).unwrap(), j);
    assert_eq!(deps_from_json(&j).unwrap(), deps);
}

#[test]
fn deps_json_rejects_bad_format() {
    assert!(deps_from_json(&json!({"kind": "field", "field": "a"})).is_err());
    assert!(deps_from_json(&json!([{"kind": "other", "field": "a"}])).is_err());
    assert!(deps_from_json(&json!([{"kind": "field"}])).is_err());
    assert!(deps_from_json(&json!([{"kind": "field", "field": "a", "x": 1}])).is_err());
    assert_eq!(deps_from_json(&json!([])).unwrap(), vec![]);
}

// ---------------------------------------------------------------------------
// 相依圖
// ---------------------------------------------------------------------------

fn master(field: &str, src: &str) -> FormulaField {
    FormulaField {
        field: FieldRef::master(field),
        deps: dependencies(&p(src)),
    }
}

fn detail(d: &str, field: &str, src: &str) -> FormulaField {
    FormulaField {
        field: FieldRef::detail(d, field),
        deps: dependencies(&p(src)),
    }
}

fn names(order: &[FieldRef]) -> Vec<String> {
    order.iter().map(ToString::to_string).collect()
}

fn position(order: &[FieldRef], name: &str) -> usize {
    order
        .iter()
        .position(|r| r.to_string() == name)
        .unwrap_or_else(|| panic!("{name} 不在 {order:?}"))
}

#[test]
fn cycle_direct() {
    let err = calculation_order(&[master("a", "{a} + 1")]).unwrap_err();
    assert_eq!(err.path, vec![FieldRef::master("a"), FieldRef::master("a")]);
    assert_eq!(err.message(), "公式有循環引用：a → a");
}

#[test]
fn cycle_indirect() {
    let err = calculation_order(&[
        master("a", "{b} + 1"),
        master("b", "{c} * 2"),
        master("c", "{a} - {x}"),
    ])
    .unwrap_err();
    assert_eq!(names(&err.path), vec!["a", "b", "c", "a"]);
    assert_eq!(err.to_string(), "公式有循環引用：a → b → c → a");
}

#[test]
fn cycle_path_excludes_prefix() {
    // x → a → b → a：路徑只包含循環的部分
    let err = calculation_order(&[master("x", "{a}"), master("a", "{b}"), master("b", "{a}")])
        .unwrap_err();
    assert_eq!(names(&err.path), vec!["a", "b", "a"]);
}

#[test]
fn cycle_through_relation_field() {
    // 關聯引用依賴本筆的關聯欄位；這裡是程式建構的相依（關聯欄位不會是公式），確認邊的對應
    let err = calculation_order(&[master("r", "{a}"), master("a", "{r.x}")]).unwrap_err();
    assert_eq!(names(&err.path), vec!["r", "a", "r"]);
}

#[test]
fn cycle_in_detail() {
    let err =
        calculation_order(&[detail("lines", "a", "{b}"), detail("lines", "b", "{a}")]).unwrap_err();
    assert_eq!(names(&err.path), vec!["lines.a", "lines.b", "lines.a"]);
}

#[test]
fn order_simple_chain() {
    let order = calculation_order(&[
        master("total", "{subtotal} + {tax}"),
        master("tax", "ROUND({subtotal} * 0.05, 0)"),
        master("subtotal", "{qty} * {price}"),
    ])
    .unwrap();
    assert_eq!(names(&order), vec!["subtotal", "tax", "total"]);
}

#[test]
fn order_detail_before_master() {
    let order = calculation_order(&[
        master("grand_total", "{subtotal} + {shipping}"),
        master("subtotal", "SUM(lines.amount)"),
        master("label", "\"x\""),
        detail("lines", "amount", "{qty} * {unit_price}"),
        detail("lines", "net", "{amount} - {discount}"),
        detail("extras", "fee", "{base} * 2"),
    ])
    .unwrap();
    assert_eq!(order.len(), 6);
    // 明細全部排在主表之前；明細子表依代碼排序
    assert_eq!(
        names(&order),
        vec![
            "extras.fee",
            "lines.amount",
            "lines.net",
            "subtotal",
            "grand_total",
            "label",
        ]
    );
    assert!(position(&order, "lines.amount") < position(&order, "subtotal"));
    assert!(position(&order, "subtotal") < position(&order, "grand_total"));
}

#[test]
fn order_same_field_name_in_master_and_detail() {
    // 主表與明細都有 amount：明細公式的 {amount} 指明細的欄位，不形成循環
    let order = calculation_order(&[
        master("amount", "SUM(lines.amount)"),
        detail("lines", "amount", "{qty} * {price}"),
        detail("lines", "double", "{amount} * 2"),
    ])
    .unwrap();
    assert_eq!(
        names(&order),
        vec!["lines.amount", "lines.double", "amount"]
    );
}

#[test]
fn order_ignores_non_formula_and_duplicates() {
    let order = calculation_order(&[
        master("a", "{qty} + {customer.rate} + SUM(lines.x)"),
        master("a", "{a}"), // 重複，忽略
    ])
    .unwrap();
    assert_eq!(order, vec![FieldRef::master("a")]);
    assert!(calculation_order(&[]).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 型別檢查
// ---------------------------------------------------------------------------

fn schema() -> TypeSchema {
    TypeSchema::master()
        .with_field("qty", DataType::Integer)
        .with_field("price", DataType::Money)
        .with_field("rate", DataType::Decimal)
        .with_field("name", DataType::Text)
        .with_field("note", DataType::LongText)
        .with_field("status", DataType::Select)
        .with_field("done", DataType::Boolean)
        .with_field("due", DataType::Date)
        .with_field("at", DataType::DateTime)
        .with_field("tags", DataType::MultiSelect)
        .with_field("photo", DataType::File)
        .with_relation(
            "customer",
            [
                ("discount_rate", DataType::Decimal),
                ("name", DataType::Text),
                ("parent", DataType::Relation),
            ],
        )
        .with_detail(
            "lines",
            [
                ("amount", DataType::Money),
                ("sku", DataType::Text),
                ("tags", DataType::MultiSelect),
            ],
        )
}

fn infer(src: &str) -> Result<ValueKind, TypeError> {
    infer_type(&p(src), &schema())
}

fn infer_err(src: &str) -> TypeError {
    infer(src).expect_err(src)
}

fn span(start: usize, end: usize) -> Span {
    Span::new(start, end)
}

#[test]
fn infer_ok() {
    let cases = [
        ("{qty} * {price}", ValueKind::Number),
        ("-{rate} / 2", ValueKind::Number),
        ("{name} & {qty}", ValueKind::Text),
        ("CONCAT({name}, {done}, {due}, {at})", ValueKind::Text),
        ("{status} = \"open\"", ValueKind::Bool),
        (
            "{done} = TRUE AND NOT ({qty} > 1) OR {due} <= TODAY()",
            ValueKind::Bool,
        ),
        ("{at} >= {at}", ValueKind::Bool),
        ("{name} < {note}", ValueKind::Bool),
        ("IF({done}, {qty}, 0)", ValueKind::Number),
        (
            "IF({status} = \"open\", DAYS(TODAY(), {due}), 0)",
            ValueKind::Number,
        ),
        ("DAYS({at}, {due})", ValueKind::Number),
        ("ROUND({price} * 0.05, 0)", ValueKind::Number),
        ("ROUND({price}, {qty})", ValueKind::Number),
        ("ABS({rate})", ValueKind::Number),
        ("TODAY()", ValueKind::Date),
        ("COALESCE({at}, {at})", ValueKind::DateTime),
        ("COALESCE({name})", ValueKind::Text),
        ("{total_x}", ValueKind::Number),
    ];
    let s = schema().with_field("total_x", DataType::Integer);
    for (src, expected) in cases {
        assert_eq!(infer_type(&p(src), &s), Ok(expected), "{src}");
    }
}

#[test]
fn infer_relation_and_aggregate() {
    assert_eq!(
        infer("{price} * (1 - {customer.discount_rate})"),
        Ok(ValueKind::Number)
    );
    assert_eq!(infer("{customer.name}"), Ok(ValueKind::Text));
    for f in ["SUM", "AVG", "MIN", "MAX", "COUNT"] {
        assert_eq!(infer(&format!("{f}(lines.amount)")), Ok(ValueKind::Number));
    }
    // COUNT 不限型態
    assert_eq!(infer("COUNT(lines.sku)"), Ok(ValueKind::Number));
}

#[test]
fn type_mismatch_points_to_operand() {
    let e = infer_err("{name} * 2");
    assert_eq!(
        e.kind,
        TypeErrorKind::TypeMismatch {
            context: "運算子「*」".to_owned(),
            expected: "數字",
            found: ValueKind::Text,
        }
    );
    assert_eq!(e.span, span(0, 6));
    assert_eq!(
        e.to_string(),
        "第 1 個字元：運算子「*」需要數字，但收到文字"
    );
    assert_eq!(e.message(), "運算子「*」需要數字，但收到文字");

    // 右邊的運算元
    let e = infer_err("1 + {done}");
    assert_eq!(e.span, span(4, 10));
}

#[test]
fn type_mismatch_cases() {
    let cases: &[(&str, &str, &str, ValueKind, Span)] = &[
        (
            "-{name}",
            "運算子「-」",
            "數字",
            ValueKind::Text,
            span(1, 7),
        ),
        (
            "NOT {qty}",
            "運算子「NOT」",
            "布林",
            ValueKind::Number,
            span(4, 9),
        ),
        (
            "{done} AND {qty}",
            "運算子「AND」",
            "布林",
            ValueKind::Number,
            span(11, 16),
        ),
        (
            "{qty} OR {done}",
            "運算子「OR」",
            "布林",
            ValueKind::Number,
            span(0, 5),
        ),
        (
            "IF({qty}, 1, 2)",
            "函式 IF 的第 1 個參數",
            "布林",
            ValueKind::Number,
            span(3, 8),
        ),
        (
            "ROUND({name}, 0)",
            "函式 ROUND 的第 1 個參數",
            "數字",
            ValueKind::Text,
            span(6, 12),
        ),
        (
            "ROUND(1, {done})",
            "函式 ROUND 的第 2 個參數",
            "數字",
            ValueKind::Bool,
            span(9, 15),
        ),
        (
            "ABS({due})",
            "函式 ABS 的第 1 個參數",
            "數字",
            ValueKind::Date,
            span(4, 9),
        ),
        (
            "DAYS({qty}, {due})",
            "函式 DAYS 的第 1 個參數",
            "日期",
            ValueKind::Number,
            span(5, 10),
        ),
        (
            "DAYS({due}, {name})",
            "函式 DAYS 的第 2 個參數",
            "日期",
            ValueKind::Text,
            span(12, 18),
        ),
    ];
    for (src, context, expected, found, sp) in cases {
        let e = infer_err(src);
        assert_eq!(
            e.kind,
            TypeErrorKind::TypeMismatch {
                context: (*context).to_owned(),
                expected,
                found: *found,
            },
            "{src}"
        );
        assert_eq!(e.span, *sp, "{src}");
    }
}

#[test]
fn incomparable() {
    for (src, op, l, r) in [
        ("{qty} = {name}", "=", ValueKind::Number, ValueKind::Text),
        ("{due} < {at}", "<", ValueKind::Date, ValueKind::DateTime),
        ("{done} > FALSE", ">", ValueKind::Bool, ValueKind::Bool),
    ] {
        let e = infer_err(src);
        assert_eq!(
            e.kind,
            TypeErrorKind::Incomparable {
                op,
                left: l,
                right: r,
            },
            "{src}"
        );
        // 位置是整個比較運算
        assert_eq!(e.span.start, 0, "{src}");
    }
    assert_eq!(infer("{done} <> FALSE"), Ok(ValueKind::Bool));
    assert_eq!(
        infer_err("{qty} = \"a\"").message(),
        "運算子「=」無法比較數字與文字"
    );
}

#[test]
fn branch_mismatch() {
    let e = infer_err("IF({done}, 1, \"a\")");
    assert_eq!(
        e.kind,
        TypeErrorKind::BranchMismatch {
            func: "IF",
            first: ValueKind::Number,
            found: ValueKind::Text,
        }
    );
    assert_eq!(e.span, span(14, 17));
    assert_eq!(
        e.message(),
        "函式 IF 的結果型態必須一致：前面是數字，這裡是文字"
    );

    let e = infer_err("COALESCE({due}, {due}, {at})");
    assert_eq!(
        e.kind,
        TypeErrorKind::BranchMismatch {
            func: "COALESCE",
            first: ValueKind::Date,
            found: ValueKind::DateTime,
        }
    );
    assert_eq!(e.span, span(23, 27));
}

#[test]
fn round_digits_constant() {
    assert_eq!(infer("ROUND(1, 28)"), Ok(ValueKind::Number));
    for src in ["ROUND(1, 29)", "ROUND(1, 0.5)"] {
        let e = infer_err(src);
        assert_eq!(e.kind, TypeErrorKind::InvalidRoundDigits, "{src}");
        assert_eq!(e.span.start, 9, "{src}");
    }
    assert!(infer_err("ROUND(1, 29)").message().contains("0 到 28"));
}

#[test]
fn round_digits_negative_constant() {
    // 解析器不會產生負數常數，程式建構的 AST 仍要拒絕
    let neg = Expr::new(ExprKind::Number(Decimal::NEGATIVE_ONE), span(9, 11));
    let expr = Expr::new(
        ExprKind::Call {
            func: Function::Round,
            args: vec![Expr::new(ExprKind::Number(Decimal::ONE), span(6, 7)), neg],
        },
        span(0, 12),
    );
    let e = infer_type(&expr, &schema()).unwrap_err();
    assert_eq!(e.kind, TypeErrorKind::InvalidRoundDigits);
}

#[test]
fn wrong_arg_count_in_built_ast() {
    let expr = Expr::new(
        ExprKind::Call {
            func: Function::If,
            args: vec![],
        },
        span(0, 4),
    );
    let e = infer_type(&expr, &schema()).unwrap_err();
    assert_eq!(
        e.kind,
        TypeErrorKind::WrongArgCount {
            func: "IF",
            expected: Arity::Exact(3),
            found: 0,
        }
    );
    assert_eq!(e.message(), "函式 IF 需要 3 個參數，但提供了 0 個");
}

#[test]
fn unknown_and_unsupported_fields() {
    let cases: Vec<(&str, TypeErrorKind)> = vec![
        ("{nope}", TypeErrorKind::UnknownField("nope".to_owned())),
        ("{nope.x}", TypeErrorKind::UnknownField("nope".to_owned())),
        ("{qty.x}", TypeErrorKind::NotRelation("qty".to_owned())),
        (
            "{customer.x}",
            TypeErrorKind::UnknownRelationField {
                relation: "customer".to_owned(),
                field: "x".to_owned(),
            },
        ),
        (
            "{tags}",
            TypeErrorKind::UnsupportedFieldType {
                field: "tags".to_owned(),
                data_type: DataType::MultiSelect,
            },
        ),
        (
            "{photo}",
            TypeErrorKind::UnsupportedFieldType {
                field: "photo".to_owned(),
                data_type: DataType::File,
            },
        ),
        (
            "{customer}",
            TypeErrorKind::UnsupportedFieldType {
                field: "customer".to_owned(),
                data_type: DataType::Relation,
            },
        ),
        (
            "{customer.parent}",
            TypeErrorKind::UnsupportedFieldType {
                field: "customer.parent".to_owned(),
                data_type: DataType::Relation,
            },
        ),
        (
            "SUM(items.x)",
            TypeErrorKind::UnknownDetail("items".to_owned()),
        ),
        (
            "SUM(lines.x)",
            TypeErrorKind::UnknownDetailField {
                detail: "lines".to_owned(),
                field: "x".to_owned(),
            },
        ),
        (
            "COUNT(lines.tags)",
            TypeErrorKind::UnsupportedFieldType {
                field: "lines.tags".to_owned(),
                data_type: DataType::MultiSelect,
            },
        ),
    ];
    for (src, kind) in cases {
        let e = infer_err(&format!("1 + {src}"));
        assert_eq!(e.kind, kind, "{src}");
        // 位置指向引用本身
        assert_eq!(e.span.start, 4, "{src}");
    }
}

#[test]
fn aggregate_requires_number_field() {
    for f in ["SUM", "AVG", "MIN", "MAX"] {
        let e = infer_err(&format!("{f}(lines.sku)"));
        assert_eq!(
            e.kind,
            TypeErrorKind::AggregateNotNumber {
                func: f,
                field: "lines.sku".to_owned(),
                found: ValueKind::Text,
            }
        );
    }
    assert_eq!(
        infer_err("SUM(lines.sku)").message(),
        "函式 SUM 只能用於數字欄位，但明細欄位「lines.sku」是文字"
    );
}

#[test]
fn error_messages() {
    let msgs = [
        ("{nope}", "找不到欄位「nope」"),
        ("{qty.x}", "欄位「qty」不是關聯欄位"),
        ("{customer.x}", "找不到關聯欄位「customer.x」"),
        ("SUM(items.x)", "找不到明細子表「items」"),
        ("SUM(lines.x)", "找不到明細欄位「lines.x」"),
        ("{tags}", "公式不能使用欄位「tags」（型態 multi_select）"),
    ];
    for (src, msg) in msgs {
        assert_eq!(infer_err(src).message(), msg, "{src}");
    }
}

// ---------------------------------------------------------------------------
// 引用規則：明細子表的公式
// ---------------------------------------------------------------------------

fn detail_schema() -> TypeSchema {
    TypeSchema::detail(["customer_name", "shipping"])
        .with_field("qty", DataType::Integer)
        .with_field("unit_price", DataType::Money)
        .with_relation("product", [("cost", DataType::Money)])
}

#[test]
fn detail_formula_ok() {
    let s = detail_schema();
    assert_eq!(
        infer_type(&p("{qty} * {unit_price} - {product.cost}"), &s),
        Ok(ValueKind::Number)
    );
}

#[test]
fn detail_formula_cannot_reference_master() {
    let s = detail_schema();
    let e = infer_type(&p("{qty} * {shipping}"), &s).unwrap_err();
    assert_eq!(
        e.kind,
        TypeErrorKind::MasterFieldInDetail("shipping".to_owned())
    );
    assert_eq!(e.span, span(8, 18));
    assert_eq!(e.message(), "明細的公式不能引用主表欄位「shipping」");

    // 關聯引用的關聯欄位在主表
    let e = infer_type(&p("{customer_name.x}"), &s).unwrap_err();
    assert_eq!(
        e.kind,
        TypeErrorKind::MasterFieldInDetail("customer_name".to_owned())
    );

    // 主表與明細都沒有的欄位
    let e = infer_type(&p("{nope}"), &s).unwrap_err();
    assert_eq!(e.kind, TypeErrorKind::UnknownField("nope".to_owned()));
}

#[test]
fn detail_formula_cannot_aggregate() {
    let s = detail_schema().with_detail("lines", [("amount", DataType::Money)]);
    let e = infer_type(&p("1 + COUNT(lines.amount)"), &s).unwrap_err();
    assert_eq!(e.kind, TypeErrorKind::AggregateInDetail { func: "COUNT" });
    assert_eq!(e.span, span(4, 23));
    assert_eq!(e.message(), "明細的公式不能使用子表彙總 COUNT");
}

#[test]
fn scope_default_is_master() {
    assert_eq!(TypeSchema::default().scope, Scope::Master);
    assert_eq!(
        TypeSchema::detail(Vec::<String>::new()).scope,
        Scope::Detail {
            master_fields: HashSet::new()
        }
    );
}

// ---------------------------------------------------------------------------
// 結果型態
// ---------------------------------------------------------------------------

#[test]
fn result_type_compatible() {
    let s = schema();
    let ok = [
        ("{qty} * 2", DataType::Integer),
        ("{qty} * 2", DataType::Decimal),
        ("{qty} * 2", DataType::Money),
        ("{name} & \"x\"", DataType::Text),
        ("{name} & \"x\"", DataType::LongText),
        ("{done}", DataType::Boolean),
        ("TODAY()", DataType::Date),
        ("{at}", DataType::DateTime),
    ];
    for (src, t) in ok {
        assert!(check_formula(&p(src), &s, t).is_ok(), "{src} → {t:?}");
    }
}

#[test]
fn result_type_incompatible() {
    let s = schema();
    let bad = [
        ("{qty} * 2", DataType::Text, ValueKind::Number),
        ("{name}", DataType::Integer, ValueKind::Text),
        ("{status}", DataType::Boolean, ValueKind::Text),
        ("{done}", DataType::Text, ValueKind::Bool),
        ("{at}", DataType::Date, ValueKind::DateTime),
        ("TODAY()", DataType::DateTime, ValueKind::Date),
    ];
    for (src, t, found) in bad {
        let expr = p(src);
        let e = check_formula(&expr, &s, t).unwrap_err();
        assert_eq!(
            e.kind,
            TypeErrorKind::ResultTypeMismatch { expected: t, found },
            "{src}"
        );
        assert_eq!(e.span, expr.span, "{src}");
    }
    assert_eq!(
        check_formula(&p("{qty} * 2"), &s, DataType::Text)
            .unwrap_err()
            .message(),
        "公式的結果是數字，無法存成 text 型態"
    );
}

#[test]
fn result_type_unsupported() {
    for t in [
        DataType::Select,
        DataType::MultiSelect,
        DataType::Relation,
        DataType::File,
        DataType::Formula,
    ] {
        let e = check_formula(&p("{name}"), &schema(), t).unwrap_err();
        assert_eq!(e.kind, TypeErrorKind::UnsupportedResultType(t));
    }
    assert_eq!(
        check_formula(&p("1"), &schema(), DataType::Select)
            .unwrap_err()
            .message(),
        "公式的結果型態不能是 select"
    );
}

#[test]
fn check_reports_inner_error_before_result_type() {
    let e = check_formula(&p("{name} * 2"), &schema(), DataType::Text).unwrap_err();
    assert!(matches!(e.kind, TypeErrorKind::TypeMismatch { .. }));
}

// ---------------------------------------------------------------------------
// 與計算一致：通過型別檢查的公式，計算時不會有型態錯誤
// ---------------------------------------------------------------------------

#[test]
fn checked_formula_evaluates() {
    struct Env;
    impl Environment for Env {
        fn today(&self) -> hammer_shared::time::Date {
            time::macros::date!(2026 - 11 - 16)
        }
        fn field(&self, key: &str) -> Option<FieldData<'_>> {
            static QTY: serde_json::Value = serde_json::Value::Null;
            (key == "qty").then(|| FieldData::new(DataType::Integer, &QTY))
        }
        fn relation_field(&self, _: &str, _: &str) -> Option<FieldData<'_>> {
            None
        }
        fn detail_column(&self, _: &str, _: &str) -> Option<DetailColumn<'_>> {
            None
        }
    }
    let s = TypeSchema::master().with_field("qty", DataType::Integer);
    let expr = p("IF({qty} > 0, DAYS(TODAY(), TODAY()), 1) & \"\"");
    assert_eq!(
        check_formula(&expr, &s, DataType::Text),
        Ok(ValueKind::Text)
    );
    assert_eq!(evaluate(&expr, &Env), Ok(Value::Text("1".to_owned())));
}
