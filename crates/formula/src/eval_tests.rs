//! 計算引擎的單元測試。

use std::collections::HashMap;
use std::str::FromStr;

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::{DataType, InvalidValue};
use hammer_shared::time::Date;
use serde_json::{Value as Json, json};
use time::macros::{date, datetime};

use crate::*;

// ---------------------------------------------------------------------------
// 測試用的計算環境
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TestEnv {
    today: Option<Date>,
    fields: HashMap<String, (DataType, Json)>,
    relations: HashMap<(String, String), (DataType, Json)>,
    details: HashMap<(String, String), (DataType, Vec<Json>)>,
}

impl TestEnv {
    fn new() -> Self {
        Self {
            today: Some(date!(2026 - 11 - 16)),
            ..Self::default()
        }
    }

    fn field(mut self, key: &str, t: DataType, v: Json) -> Self {
        self.fields.insert(key.to_owned(), (t, v));
        self
    }

    fn relation(mut self, rel: &str, key: &str, t: DataType, v: Json) -> Self {
        self.relations
            .insert((rel.to_owned(), key.to_owned()), (t, v));
        self
    }

    fn detail(mut self, detail: &str, key: &str, t: DataType, rows: Vec<Json>) -> Self {
        self.details
            .insert((detail.to_owned(), key.to_owned()), (t, rows));
        self
    }
}

impl Environment for TestEnv {
    fn today(&self) -> Date {
        self.today.unwrap_or(date!(2000 - 01 - 01))
    }

    fn field(&self, key: &str) -> Option<FieldData<'_>> {
        self.fields.get(key).map(|(t, v)| FieldData::new(*t, v))
    }

    fn relation_field(&self, relation: &str, field: &str) -> Option<FieldData<'_>> {
        self.relations
            .get(&(relation.to_owned(), field.to_owned()))
            .map(|(t, v)| FieldData::new(*t, v))
    }

    fn detail_column(&self, detail: &str, field: &str) -> Option<DetailColumn<'_>> {
        self.details
            .get(&(detail.to_owned(), field.to_owned()))
            .map(|(t, rows)| DetailColumn {
                data_type: *t,
                values: rows.iter().collect(),
            })
    }
}

fn calc(src: &str, env: &TestEnv) -> Result<Value, EvalError> {
    let expr = parse(src).unwrap_or_else(|e| panic!("{src} 應該能解析：{e}"));
    evaluate(&expr, env)
}

/// 沒有欄位的環境計算。
fn calc0(src: &str) -> Result<Value, EvalError> {
    calc(src, &TestEnv::new())
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn n(s: &str) -> Value {
    Value::Number(d(s))
}

fn t(s: &str) -> Value {
    Value::Text(s.to_owned())
}

fn b(v: bool) -> Value {
    Value::Bool(v)
}

/// 型態不符的錯誤（只比較預期的型態與收到的值）。
fn assert_mismatch(src: &str, env: &TestEnv, expected: &str, found: ValueKind) {
    match calc(src, env) {
        Err(EvalError::TypeMismatch {
            expected: e,
            found: f,
            ..
        }) => {
            assert_eq!((e, f), (expected, found), "{src}");
        }
        other => panic!("{src} 應該是型態不符，實際為 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// §7.6 的語法範例
// ---------------------------------------------------------------------------

fn spec_env() -> TestEnv {
    TestEnv::new()
        .field("qty", DataType::Integer, json!(3))
        .field("unit_price", DataType::Money, json!("12.50"))
        .field("subtotal", DataType::Money, json!("1234.00"))
        .field("shipping_fee", DataType::Money, json!("60"))
        .field("total", DataType::Money, json!("1000"))
        .field("status", DataType::Select, json!("open"))
        .field("due_date", DataType::Date, json!("2026-11-01"))
        .relation(
            "customer",
            "discount_rate",
            DataType::Decimal,
            json!("0.15"),
        )
        .detail(
            "lines",
            "amount",
            DataType::Money,
            vec![json!("100.25"), json!("200.75"), json!(null)],
        )
}

#[test]
fn spec_examples() {
    let env = spec_env();
    assert_eq!(calc("{qty} * {unit_price}", &env), Ok(n("37.50")));
    // 1234 × 0.05 = 61.7 → 62
    assert_eq!(calc("ROUND({subtotal} * 0.05, 0)", &env), Ok(n("62")));
    assert_eq!(
        calc("SUM(lines.amount) + {shipping_fee}", &env),
        Ok(n("361"))
    );
    assert_eq!(
        calc("{total} * (1 - {customer.discount_rate})", &env),
        Ok(n("850"))
    );
    // 2026-11-16 − 2026-11-01 = 15 天
    assert_eq!(
        calc(
            "IF({status} = \"open\", DAYS(TODAY(), {due_date}), 0)",
            &env
        ),
        Ok(n("15"))
    );
    let closed = spec_env().field("status", DataType::Select, json!("closed"));
    assert_eq!(
        calc(
            "IF({status} = \"open\", DAYS(TODAY(), {due_date}), 0)",
            &closed
        ),
        Ok(n("0"))
    );
}

// ---------------------------------------------------------------------------
// 精度
// ---------------------------------------------------------------------------

#[test]
fn decimal_arithmetic_is_exact() {
    assert_eq!(calc0("0.1 + 0.2 = 0.3"), Ok(b(true)));
    assert_eq!(calc0("0.1 + 0.2"), Ok(n("0.3")));
    assert_eq!(calc0("1 - 0.9"), Ok(n("0.1")));
    assert_eq!(calc0("1.1 * 1.1"), Ok(n("1.21")));
    // 大數與高精度小數
    assert_eq!(
        calc0("99999999999999.9999 + 0.0001"),
        Ok(n("100000000000000.0000"))
    );
    assert_eq!(
        calc0("12345678901234567890 * 10"),
        Ok(n("123456789012345678900"))
    );
    assert_eq!(
        calc0("0.0000000000000000000000000001 * 2"),
        Ok(n("0.0000000000000000000000000002"))
    );
    assert_eq!(calc0("1234567.123456 + 0.000001"), Ok(n("1234567.123457")));
    let env = TestEnv::new().field(
        "big",
        DataType::Decimal,
        json!("79228162514264337593543950335"),
    );
    assert_eq!(
        calc("{big} - 1", &env),
        Ok(n("79228162514264337593543950334"))
    );
}

// ---------------------------------------------------------------------------
// 算術運算子
// ---------------------------------------------------------------------------

#[test]
fn arithmetic_operators() {
    assert_eq!(calc0("1 + 2"), Ok(n("3")));
    assert_eq!(calc0("1 - 2"), Ok(n("-1")));
    assert_eq!(calc0("2.5 * 4"), Ok(n("10.0")));
    assert_eq!(calc0("10 / 4"), Ok(n("2.5")));
    assert_eq!(calc0("-3 * 2"), Ok(n("-6")));
    assert_eq!(calc0("--3"), Ok(n("3")));
    // 除不盡時取 Decimal 可表示的精度
    assert_eq!(calc0("1 / 3"), Ok(n("0.3333333333333333333333333333")));
    assert_eq!(calc0("0 / 5"), Ok(n("0")));
}

#[test]
fn arithmetic_with_null_is_null() {
    let env = TestEnv::new().field("x", DataType::Decimal, json!(null));
    for src in [
        "{x} + 1", "1 - {x}", "{x} * 2", "{x} / 2", "2 / {x}", "-{x}",
    ] {
        assert_eq!(calc(src, &env), Ok(Value::Null), "{src}");
    }
}

#[test]
fn division_by_zero() {
    assert_eq!(calc0("1 / 0"), Err(EvalError::DivisionByZero));
    assert_eq!(calc0("1 / 0.000"), Err(EvalError::DivisionByZero));
    let env = TestEnv::new().field("qty", DataType::Integer, json!(0));
    assert_eq!(calc("10 / {qty}", &env), Err(EvalError::DivisionByZero));
}

#[test]
fn overflow() {
    let max = "79228162514264337593543950335";
    assert_eq!(calc0(&format!("{max} + 1")), Err(EvalError::Overflow));
    assert_eq!(calc0(&format!("-{max} - 1")), Err(EvalError::Overflow));
    assert_eq!(calc0(&format!("{max} * 2")), Err(EvalError::Overflow));
    assert_eq!(calc0(&format!("{max} / 0.1")), Err(EvalError::Overflow));
}

#[test]
fn arithmetic_type_mismatch() {
    let env = TestEnv::new().field("x", DataType::Decimal, json!(null));
    assert_mismatch("\"a\" * 2", &env, "數字", ValueKind::Text);
    assert_mismatch("2 + TRUE", &env, "數字", ValueKind::Bool);
    assert_mismatch("TODAY() - 1", &env, "數字", ValueKind::Date);
    assert_mismatch("-\"a\"", &env, "數字", ValueKind::Text);
    // 型態不符優先於空值
    assert_mismatch("\"a\" * {x}", &env, "數字", ValueKind::Text);
    assert_eq!(
        calc0("\"a\" * 2").unwrap_err().message(),
        "運算子「*」需要數字，但收到文字"
    );
}

// ---------------------------------------------------------------------------
// 字串連接
// ---------------------------------------------------------------------------

#[test]
fn concat_operator() {
    assert_eq!(calc0("\"A\" & \"B\""), Ok(t("AB")));
    assert_eq!(calc0("\"數量：\" & 12.50"), Ok(t("數量：12.50")));
    assert_eq!(calc0("TRUE & \"/\" & FALSE"), Ok(t("TRUE/FALSE")));
    assert_eq!(calc0("\"今天 \" & TODAY()"), Ok(t("今天 2026-11-16")));
    let env = TestEnv::new()
        .field("x", DataType::Text, json!(null))
        .field("at", DataType::DateTime, json!("2026-11-16T09:30:00+08:00"));
    // 空值視為空字串
    assert_eq!(calc("\"a\" & {x} & \"b\"", &env), Ok(t("ab")));
    assert_eq!(calc("{x} & {x}", &env), Ok(t("")));
    assert_eq!(
        calc("{at} & \"\"", &env),
        Ok(t("2026-11-16T09:30:00+08:00"))
    );
}

// ---------------------------------------------------------------------------
// 比較運算子
// ---------------------------------------------------------------------------

#[test]
fn comparison_operators() {
    let cases = [
        ("1 = 1.00", true),
        ("1 <> 1.00", false),
        ("1 < 2", true),
        ("2 < 2", false),
        ("2 <= 2", true),
        ("3 <= 2", false),
        ("3 > 2", true),
        ("2 > 2", false),
        ("2 >= 2", true),
        ("1 >= 2", false),
        ("-1 < 0", true),
        ("\"a\" = \"a\"", true),
        ("\"a\" = \"A\"", false),
        ("\"a\" < \"b\"", true),
        ("\"甲\" <> \"乙\"", true),
        ("TRUE = TRUE", true),
        ("TRUE <> FALSE", true),
        ("TODAY() = TODAY()", true),
    ];
    for (src, expected) in cases {
        assert_eq!(calc0(src), Ok(b(expected)), "{src}");
    }

    let env = TestEnv::new()
        .field("a", DataType::Date, json!("2026-01-01"))
        .field("b", DataType::Date, json!("2026-12-31"))
        .field("t1", DataType::DateTime, json!("2026-11-16T09:00:00+08:00"))
        .field("t2", DataType::DateTime, json!("2026-11-16T01:00:00Z"));
    assert_eq!(calc("{a} < {b}", &env), Ok(b(true)));
    assert_eq!(calc("{a} >= {b}", &env), Ok(b(false)));
    // 同一個時間點，時區位移不同
    assert_eq!(calc("{t1} = {t2}", &env), Ok(b(true)));
    assert_eq!(calc("{t1} < {t2}", &env), Ok(b(false)));
}

#[test]
fn comparison_with_null_is_null() {
    let env = TestEnv::new().field("x", DataType::Integer, json!(null));
    for src in [
        "{x} = 1",
        "1 <> {x}",
        "{x} < 1",
        "{x} >= {x}",
        "{x} = \"a\"",
    ] {
        assert_eq!(calc(src, &env), Ok(Value::Null), "{src}");
    }
}

#[test]
fn comparison_of_different_types() {
    assert_eq!(
        calc0("1 = \"1\""),
        Err(EvalError::Incomparable {
            op: "=",
            left: ValueKind::Number,
            right: ValueKind::Text,
        })
    );
    assert_eq!(
        calc0("TRUE < FALSE"),
        Err(EvalError::Incomparable {
            op: "<",
            left: ValueKind::Bool,
            right: ValueKind::Bool,
        })
    );
    let env = TestEnv::new()
        .field("d", DataType::Date, json!("2026-11-16"))
        .field("dt", DataType::DateTime, json!("2026-11-16T00:00:00+08:00"));
    assert!(matches!(
        calc("{d} = {dt}", &env),
        Err(EvalError::Incomparable { .. })
    ));
    assert_eq!(
        calc0("1 = \"1\"").unwrap_err().message(),
        "運算子「=」無法比較數字與文字"
    );
}

// ---------------------------------------------------------------------------
// 邏輯運算子
// ---------------------------------------------------------------------------

#[test]
fn logical_operators() {
    assert_eq!(calc0("TRUE AND TRUE"), Ok(b(true)));
    assert_eq!(calc0("TRUE AND FALSE"), Ok(b(false)));
    assert_eq!(calc0("FALSE OR TRUE"), Ok(b(true)));
    assert_eq!(calc0("FALSE OR FALSE"), Ok(b(false)));
    assert_eq!(calc0("NOT TRUE"), Ok(b(false)));
    assert_eq!(calc0("NOT NOT TRUE"), Ok(b(true)));
    assert_eq!(calc0("1 < 2 AND 2 < 3"), Ok(b(true)));
}

#[test]
fn logical_three_valued_with_null() {
    let env = TestEnv::new().field("x", DataType::Boolean, json!(null));
    let cases = [
        ("{x} AND FALSE", b(false)),
        ("FALSE AND {x}", b(false)),
        ("{x} AND TRUE", Value::Null),
        ("{x} OR TRUE", b(true)),
        ("TRUE OR {x}", b(true)),
        ("{x} OR FALSE", Value::Null),
        ("NOT {x}", Value::Null),
    ];
    for (src, expected) in cases {
        assert_eq!(calc(src, &env), Ok(expected), "{src}");
    }
}

#[test]
fn logical_type_mismatch() {
    let env = TestEnv::new();
    assert_mismatch("1 AND TRUE", &env, "布林", ValueKind::Number);
    assert_mismatch("TRUE OR \"a\"", &env, "布林", ValueKind::Text);
    assert_mismatch("NOT 1", &env, "布林", ValueKind::Number);
}

// ---------------------------------------------------------------------------
// 函式
// ---------------------------------------------------------------------------

#[test]
fn if_function() {
    assert_eq!(calc0("IF(TRUE, 1, 2)"), Ok(n("1")));
    assert_eq!(calc0("IF(FALSE, 1, 2)"), Ok(n("2")));
    assert_eq!(calc0("IF(1 > 2, \"大\", \"小\")"), Ok(t("小")));
    // 空值條件取「否」
    let env = TestEnv::new()
        .field("flag", DataType::Boolean, json!(null))
        .field("qty", DataType::Integer, json!(0))
        .field("total", DataType::Money, json!("100"));
    assert_eq!(calc("IF({flag}, 1, 2)", &env), Ok(n("2")));
    // 只計算選到的分支
    assert_eq!(calc("IF({qty} = 0, 0, {total} / {qty})", &env), Ok(n("0")));
    assert_eq!(calc("IF(TRUE, 1, 1 / 0)", &env), Ok(n("1")));
    assert_eq!(calc("IF(FALSE, 1 / 0, 2)", &env), Ok(n("2")));
    // 條件不是布林
    assert_mismatch("IF(1, 2, 3)", &env, "布林", ValueKind::Number);
    assert_eq!(
        calc0("IF(\"a\", 2, 3)").unwrap_err().message(),
        "函式 IF 的第 1 個參數需要布林，但收到文字"
    );
}

#[test]
fn round_function() {
    assert_eq!(calc0("ROUND(2.5, 0)"), Ok(n("3")));
    assert_eq!(calc0("ROUND(-2.5, 0)"), Ok(n("-3")));
    assert_eq!(calc0("ROUND(1.2345, 2)"), Ok(n("1.23")));
    assert_eq!(calc0("ROUND(1.235, 2)"), Ok(n("1.24")));
    assert_eq!(calc0("ROUND(1.5, 3)"), Ok(n("1.5")));
    assert_eq!(
        calc0("ROUND(0.0000000000000000000000000005, 27)"),
        Ok(n("0.000000000000000000000000001"))
    );
    assert_eq!(calc0("ROUND(1, 28)"), Ok(n("1")));
    assert_eq!(calc0("ROUND(1.5, 0.0)"), Ok(n("2")));

    let env = TestEnv::new().field("x", DataType::Decimal, json!(null));
    assert_eq!(calc("ROUND({x}, 2)", &env), Ok(Value::Null));
    assert_eq!(calc("ROUND(1.5, {x})", &env), Ok(Value::Null));

    for src in ["ROUND(1, -1)", "ROUND(1, 1.5)", "ROUND(1, 29)"] {
        assert_eq!(calc0(src), Err(EvalError::InvalidRoundDigits), "{src}");
    }
    assert_mismatch("ROUND(\"a\", 2)", &env, "數字", ValueKind::Text);
    assert_mismatch("ROUND(1, \"2\")", &env, "數字", ValueKind::Text);
    assert_eq!(
        calc0("ROUND(1, 29)").unwrap_err().message(),
        "函式 ROUND 的小數位數必須是 0 到 28 的整數"
    );
}

#[test]
fn abs_function() {
    assert_eq!(calc0("ABS(-1.50)"), Ok(n("1.50")));
    assert_eq!(calc0("ABS(2)"), Ok(n("2")));
    assert_eq!(calc0("ABS(0)"), Ok(n("0")));
    let env = TestEnv::new().field("x", DataType::Decimal, json!(null));
    assert_eq!(calc("ABS({x})", &env), Ok(Value::Null));
    assert_mismatch("ABS(TRUE)", &env, "數字", ValueKind::Bool);
}

#[test]
fn today_function() {
    assert_eq!(calc0("TODAY()"), Ok(Value::Date(date!(2026 - 11 - 16))));
    let env = TestEnv {
        today: Some(date!(2024 - 02 - 29)),
        ..TestEnv::default()
    };
    assert_eq!(
        calc("TODAY()", &env),
        Ok(Value::Date(date!(2024 - 02 - 29)))
    );
}

#[test]
fn days_function() {
    let env = TestEnv::new()
        .field("start", DataType::Date, json!("2026-01-01"))
        .field("end", DataType::Date, json!("2026-12-31"))
        .field("leap", DataType::Date, json!("2024-03-01"))
        .field("none", DataType::Date, json!(null))
        // UTC 16:30 是台北隔天 00:30
        .field("at", DataType::DateTime, json!("2026-11-15T16:30:00Z"));
    assert_eq!(calc("DAYS({end}, {start})", &env), Ok(n("364")));
    assert_eq!(calc("DAYS({start}, {end})", &env), Ok(n("-364")));
    assert_eq!(calc("DAYS({start}, {start})", &env), Ok(n("0")));
    assert_eq!(
        calc("DAYS({leap}, {leap}) + DAYS(TODAY(), {leap})", &env),
        Ok(n("990"))
    );
    // 日期時間以 Asia/Taipei 的日期計算
    assert_eq!(calc("DAYS(TODAY(), {at})", &env), Ok(n("0")));
    assert_eq!(calc("DAYS({none}, {start})", &env), Ok(Value::Null));
    assert_eq!(calc("DAYS({start}, {none})", &env), Ok(Value::Null));
    assert_mismatch("DAYS(1, {start})", &env, "日期", ValueKind::Number);
    assert_mismatch(
        "DAYS({start}, \"2026-01-01\")",
        &env,
        "日期",
        ValueKind::Text,
    );
}

#[test]
fn concat_function() {
    assert_eq!(calc0("CONCAT(\"A\")"), Ok(t("A")));
    assert_eq!(calc0("CONCAT(\"A\", 1, TRUE)"), Ok(t("A1TRUE")));
    let env = TestEnv::new().field("x", DataType::Text, json!(null));
    assert_eq!(calc("CONCAT({x}, \"b\", {x})", &env), Ok(t("b")));
    assert_eq!(calc("CONCAT({x})", &env), Ok(t("")));
    // 參數計算失敗時整個失敗
    assert_eq!(
        calc0("CONCAT(\"a\", 1 / 0)"),
        Err(EvalError::DivisionByZero)
    );
}

#[test]
fn coalesce_function() {
    let env = TestEnv::new()
        .field("a", DataType::Decimal, json!(null))
        .field("b", DataType::Decimal, json!("2.5"));
    assert_eq!(calc("COALESCE({a}, {b}, 9)", &env), Ok(n("2.5")));
    assert_eq!(calc("COALESCE({b}, {a})", &env), Ok(n("2.5")));
    assert_eq!(calc("COALESCE({a})", &env), Ok(Value::Null));
    assert_eq!(calc("COALESCE({a}, {a})", &env), Ok(Value::Null));
    assert_eq!(calc("COALESCE({a}, 0) + 1", &env), Ok(n("1")));
    // 找到非空值後不再計算後面的參數
    assert_eq!(calc("COALESCE({b}, 1 / 0)", &env), Ok(n("2.5")));
    assert_eq!(
        calc("COALESCE({a}, 1 / 0)", &env),
        Err(EvalError::DivisionByZero)
    );
}

// ---------------------------------------------------------------------------
// 明細彙總
// ---------------------------------------------------------------------------

fn lines_env(rows: Vec<Json>) -> TestEnv {
    TestEnv::new().detail("lines", "amount", DataType::Money, rows)
}

#[test]
fn aggregates_with_values() {
    let env = lines_env(vec![json!("10.50"), json!("-2"), json!("30")]);
    assert_eq!(calc("SUM(lines.amount)", &env), Ok(n("38.50")));
    assert_eq!(
        calc("AVG(lines.amount)", &env),
        Ok(n("12.833333333333333333333333333"))
    );
    assert_eq!(calc("MIN(lines.amount)", &env), Ok(n("-2")));
    assert_eq!(calc("MAX(lines.amount)", &env), Ok(n("30")));
    assert_eq!(calc("COUNT(lines.amount)", &env), Ok(n("3")));

    let ints = TestEnv::new().detail("lines", "qty", DataType::Integer, vec![json!(2), json!(4)]);
    assert_eq!(calc("SUM(lines.qty)", &ints), Ok(n("6")));
    assert_eq!(calc("AVG(lines.qty)", &ints), Ok(n("3")));
}

#[test]
fn aggregates_without_rows() {
    let env = lines_env(vec![]);
    assert_eq!(calc("SUM(lines.amount)", &env), Ok(n("0")));
    assert_eq!(calc("COUNT(lines.amount)", &env), Ok(n("0")));
    assert_eq!(calc("AVG(lines.amount)", &env), Ok(Value::Null));
    assert_eq!(calc("MIN(lines.amount)", &env), Ok(Value::Null));
    assert_eq!(calc("MAX(lines.amount)", &env), Ok(Value::Null));
    assert_eq!(calc("SUM(lines.amount) + 5", &env), Ok(n("5")));
}

#[test]
fn aggregates_ignore_nulls() {
    let env = lines_env(vec![json!("10"), json!(null), json!("20")]);
    assert_eq!(calc("SUM(lines.amount)", &env), Ok(n("30")));
    assert_eq!(calc("AVG(lines.amount)", &env), Ok(n("15")));
    assert_eq!(calc("MIN(lines.amount)", &env), Ok(n("10")));
    assert_eq!(calc("MAX(lines.amount)", &env), Ok(n("20")));
    // COUNT 計算列數（含空值的列）
    assert_eq!(calc("COUNT(lines.amount)", &env), Ok(n("3")));

    let all_null = lines_env(vec![json!(null), json!(null)]);
    assert_eq!(calc("SUM(lines.amount)", &all_null), Ok(n("0")));
    assert_eq!(calc("AVG(lines.amount)", &all_null), Ok(Value::Null));
    assert_eq!(calc("MIN(lines.amount)", &all_null), Ok(Value::Null));
    assert_eq!(calc("MAX(lines.amount)", &all_null), Ok(Value::Null));
    assert_eq!(calc("COUNT(lines.amount)", &all_null), Ok(n("2")));
}

#[test]
fn aggregates_overflow() {
    let max = "79228162514264337593543950335";
    let env = lines_env(vec![json!(max), json!("1")]);
    assert_eq!(calc("SUM(lines.amount)", &env), Err(EvalError::Overflow));
    assert_eq!(calc("AVG(lines.amount)", &env), Err(EvalError::Overflow));
    assert_eq!(calc("MAX(lines.amount)", &env), Ok(n(max)));
}

#[test]
fn aggregates_require_numeric_field() {
    let env = TestEnv::new()
        .detail(
            "lines",
            "name",
            DataType::Text,
            vec![json!(null), json!("螺絲")],
        )
        .detail("lines", "note", DataType::Text, vec![])
        .detail("lines", "day", DataType::Date, vec![json!("2026-11-16")]);
    assert_mismatch("SUM(lines.name)", &env, "數字", ValueKind::Text);
    assert_mismatch("AVG(lines.day)", &env, "數字", ValueKind::Date);
    // 沒有明細時也一樣是型態錯誤
    assert_mismatch("MAX(lines.note)", &env, "數字", ValueKind::Null);
    // COUNT 不限型態
    assert_eq!(calc("COUNT(lines.name)", &env), Ok(n("2")));
    assert_eq!(
        calc("SUM(lines.name)", &env).unwrap_err().message(),
        "函式 SUM 的明細欄位「lines.name」需要數字，但收到文字"
    );
}

// ---------------------------------------------------------------------------
// 欄位值的轉換
// ---------------------------------------------------------------------------

#[test]
fn field_values_from_json() {
    let env = TestEnv::new()
        .field("i", DataType::Integer, json!(-42))
        .field("dec", DataType::Decimal, json!("12.3400"))
        .field("m", DataType::Money, json!("0.5"))
        .field("txt", DataType::Text, json!("abc"))
        .field("long", DataType::LongText, json!("第一行\n第二行"))
        .field("sel", DataType::Select, json!("open"))
        .field("ok", DataType::Boolean, json!(true))
        .field("day", DataType::Date, json!("2026-11-16"))
        .field("at", DataType::DateTime, json!("2026-11-16T09:30:00+08:00"))
        .field("empty", DataType::Text, json!(null));
    assert_eq!(calc("{i}", &env), Ok(n("-42")));
    assert_eq!(calc("{dec}", &env), Ok(n("12.3400")));
    assert_eq!(calc("{m}", &env), Ok(n("0.5")));
    assert_eq!(calc("{txt}", &env), Ok(t("abc")));
    assert_eq!(calc("{long}", &env), Ok(t("第一行\n第二行")));
    assert_eq!(calc("{sel}", &env), Ok(t("open")));
    assert_eq!(calc("{ok}", &env), Ok(b(true)));
    assert_eq!(calc("{day}", &env), Ok(Value::Date(date!(2026 - 11 - 16))));
    assert_eq!(
        calc("{at}", &env),
        Ok(Value::DateTime(datetime!(2026-11-16 09:30 +8)))
    );
    assert_eq!(calc("{empty}", &env), Ok(Value::Null));
}

#[test]
fn field_values_unsupported_or_invalid() {
    let env = TestEnv::new()
        .field("tags", DataType::MultiSelect, json!(["a"]))
        .field("cust", DataType::Relation, json!(null))
        .field("doc", DataType::File, json!(null))
        .field("f", DataType::Formula, json!("1"))
        .field("bad", DataType::Decimal, json!(12.5))
        .field("bad_date", DataType::Date, json!("2026/11/16"));
    for (src, key, t) in [
        ("{tags}", "tags", DataType::MultiSelect),
        ("{cust}", "cust", DataType::Relation),
        ("{doc}", "doc", DataType::File),
        ("{f}", "f", DataType::Formula),
    ] {
        assert_eq!(
            calc(src, &env),
            Err(EvalError::UnsupportedFieldType {
                field: key.to_owned(),
                data_type: t,
            })
        );
    }
    assert_eq!(
        calc("{bad} + 1", &env),
        Err(EvalError::InvalidStoredValue {
            field: "bad".to_owned(),
            reason: InvalidValue::TypeMismatch(DataType::Decimal),
        })
    );
    assert_eq!(
        calc("{bad_date}", &env),
        Err(EvalError::InvalidStoredValue {
            field: "bad_date".to_owned(),
            reason: InvalidValue::InvalidDate,
        })
    );
    assert_eq!(
        calc("{tags}", &env).unwrap_err().message(),
        "公式不能使用欄位「tags」（型態 multi_select）"
    );
    assert_eq!(
        calc("{bad_date}", &env).unwrap_err().message(),
        "欄位「bad_date」的值不正確：日期格式不正確，必須是 YYYY-MM-DD"
    );
}

#[test]
fn unknown_fields() {
    let env = TestEnv::new();
    assert_eq!(
        calc("{nope}", &env),
        Err(EvalError::UnknownField("nope".to_owned()))
    );
    assert_eq!(
        calc("{customer.rate}", &env),
        Err(EvalError::UnknownRelationField {
            relation: "customer".to_owned(),
            field: "rate".to_owned(),
        })
    );
    assert_eq!(
        calc("SUM(lines.amount)", &env),
        Err(EvalError::UnknownDetailField {
            detail: "lines".to_owned(),
            field: "amount".to_owned(),
        })
    );
    assert_eq!(
        calc("{nope}", &env).unwrap_err().message(),
        "找不到欄位「nope」"
    );
    assert_eq!(
        calc("{customer.rate}", &env).unwrap_err().message(),
        "找不到關聯欄位「customer.rate」"
    );
    assert_eq!(
        calc("SUM(lines.amount)", &env).unwrap_err().message(),
        "找不到明細欄位「lines.amount」"
    );
}

#[test]
fn relation_without_target_is_null() {
    struct NoTarget;
    impl Environment for NoTarget {
        fn today(&self) -> Date {
            date!(2026 - 11 - 16)
        }
        fn field(&self, _: &str) -> Option<FieldData<'_>> {
            None
        }
        fn relation_field(&self, _: &str, _: &str) -> Option<FieldData<'_>> {
            Some(FieldData::null(DataType::Decimal))
        }
        fn detail_column(&self, _: &str, _: &str) -> Option<DetailColumn<'_>> {
            None
        }
    }
    let expr = parse("COALESCE({customer.discount_rate}, 0)").unwrap();
    assert_eq!(evaluate(&expr, &NoTarget), Ok(n("0")));
    let expr = parse("{customer.discount_rate} * 2").unwrap();
    assert_eq!(evaluate(&expr, &NoTarget), Ok(Value::Null));
}

// ---------------------------------------------------------------------------
// 轉成 JSON 存值
// ---------------------------------------------------------------------------

#[test]
fn storage_decimal_rounds_and_pads() {
    for t in [DataType::Decimal, DataType::Money] {
        let cases = [
            ("1.23456", 4, "1.2346"),
            ("1.23455", 4, "1.2346"),
            ("-1.23455", 4, "-1.2346"),
            ("12.3", 4, "12.3000"),
            ("12", 2, "12.00"),
            ("2.5", 0, "3"),
            ("-2.5", 0, "-3"),
            ("0.1", 6, "0.100000"),
            ("1234567890.123", 2, "1234567890.12"),
        ];
        for (v, p, expected) in cases {
            assert_eq!(
                to_storage(&n(v), t, Some(p)),
                Ok(json!(expected)),
                "{v} {p}"
            );
        }
    }
    assert_eq!(
        to_storage(&n("1"), DataType::Decimal, None),
        Err(EvalError::InvalidDecimalPlaces(None))
    );
    assert_eq!(
        to_storage(&n("1"), DataType::Decimal, Some(-1)),
        Err(EvalError::InvalidDecimalPlaces(Some(-1)))
    );
    assert_eq!(
        to_storage(&n("1"), DataType::Money, Some(29)),
        Err(EvalError::InvalidDecimalPlaces(Some(29)))
    );
    // 整數位數太多，補不了小數位數
    assert_eq!(
        to_storage(
            &n("79228162514264337593543950335"),
            DataType::Decimal,
            Some(2)
        ),
        Err(EvalError::Overflow)
    );
    assert_eq!(
        to_storage(&n("1"), DataType::Decimal, None)
            .unwrap_err()
            .message(),
        "公式的結果沒有設定小數位數"
    );
    assert_eq!(
        to_storage(&n("1"), DataType::Decimal, Some(29))
            .unwrap_err()
            .message(),
        "公式結果的小數位數 29 不正確，必須介於 0 到 28"
    );
}

#[test]
fn storage_integer_rounds() {
    assert_eq!(to_storage(&n("2.5"), DataType::Integer, None), Ok(json!(3)));
    assert_eq!(
        to_storage(&n("-2.5"), DataType::Integer, None),
        Ok(json!(-3))
    );
    assert_eq!(to_storage(&n("7"), DataType::Integer, None), Ok(json!(7)));
    assert_eq!(
        to_storage(&n("9007199254740991"), DataType::Integer, None),
        Ok(json!(9_007_199_254_740_991_i64))
    );
    assert_eq!(
        to_storage(&n("9007199254740992"), DataType::Integer, None),
        Err(EvalError::Overflow)
    );
}

#[test]
fn storage_other_types() {
    assert_eq!(
        to_storage(&Value::Date(date!(2026 - 01 - 05)), DataType::Date, None),
        Ok(json!("2026-01-05"))
    );
    assert_eq!(
        to_storage(
            &Value::DateTime(datetime!(2026-11-16 09:30 +8)),
            DataType::DateTime,
            None
        ),
        Ok(json!("2026-11-16T09:30:00+08:00"))
    );
    assert_eq!(
        to_storage(&t("abc"), DataType::Text, None),
        Ok(json!("abc"))
    );
    assert_eq!(
        to_storage(&t("a\nb"), DataType::LongText, None),
        Ok(json!("a\nb"))
    );
    assert_eq!(
        to_storage(&b(false), DataType::Boolean, None),
        Ok(json!(false))
    );
    // 空值一律是 null
    for rt in [
        DataType::Integer,
        DataType::Decimal,
        DataType::Text,
        DataType::Boolean,
        DataType::Date,
        DataType::DateTime,
    ] {
        assert_eq!(
            to_storage(&Value::Null, rt, Some(2)),
            Ok(Json::Null),
            "{rt:?}"
        );
    }
}

#[test]
fn storage_type_mismatch() {
    assert_eq!(
        to_storage(&t("a"), DataType::Decimal, Some(2)),
        Err(EvalError::ResultTypeMismatch {
            expected: DataType::Decimal,
            found: ValueKind::Text,
        })
    );
    assert!(to_storage(&n("1"), DataType::Text, None).is_err());
    assert!(
        to_storage(
            &Value::Date(date!(2026 - 01 - 05)),
            DataType::DateTime,
            None
        )
        .is_err()
    );
    assert_eq!(
        to_storage(&n("1"), DataType::Boolean, None)
            .unwrap_err()
            .message(),
        "計算結果是數字，無法存成 boolean 型態"
    );
    for rt in [
        DataType::Select,
        DataType::MultiSelect,
        DataType::Relation,
        DataType::File,
        DataType::Formula,
    ] {
        assert_eq!(
            to_storage(&Value::Null, rt, None),
            Err(EvalError::UnsupportedResultType(rt))
        );
    }
    assert_eq!(
        to_storage(&t("a"), DataType::Select, None)
            .unwrap_err()
            .message(),
        "公式的結果型態不能是 select"
    );
}

#[test]
fn compute_returns_value_or_error() {
    let env = spec_env();
    let expr = parse("{qty} * {unit_price}").unwrap();
    assert_eq!(
        compute(&expr, &env, DataType::Money, Some(2)),
        Computed {
            value: json!("37.50"),
            error: None,
        }
    );
    // 除以零、型態不符、溢位：值為 null 並帶錯誤原因
    for (src, error) in [
        ("{total} / 0", "除數不可為零"),
        ("\"a\" * 2", "運算子「*」需要數字，但收到文字"),
        (
            "79228162514264337593543950335 * {qty}",
            "計算結果超出可表示的範圍",
        ),
    ] {
        let result = compute(&parse(src).unwrap(), &env, DataType::Decimal, Some(2));
        assert_eq!(result.value, Json::Null, "{src}");
        assert_eq!(
            result.error.map(|e| e.message()).as_deref(),
            Some(error),
            "{src}"
        );
    }
    // 結果型態不符也是錯誤
    let result = compute(&parse("\"a\"").unwrap(), &env, DataType::Integer, None);
    assert_eq!(result.value, Json::Null);
    assert!(result.error.is_some());
}

#[test]
fn value_kind_names() {
    let names: Vec<_> = [
        Value::Null,
        n("1"),
        t("a"),
        b(true),
        Value::Date(date!(2026 - 01 - 01)),
        Value::DateTime(datetime!(2026-01-01 0:00 UTC)),
    ]
    .iter()
    .map(|v| v.kind().to_string())
    .collect();
    assert_eq!(names, ["空值", "數字", "文字", "布林", "日期", "日期時間"]);
}
