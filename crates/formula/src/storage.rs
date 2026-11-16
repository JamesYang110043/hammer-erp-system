//! 計算結果轉成 §7.3 的 JSON 存值格式。

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::value::MAX_SAFE_INTEGER;
use hammer_shared::schema::{DataType, FieldValue, to_json};
use rust_decimal::RoundingStrategy;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value as Json;

use crate::ast::Expr;
use crate::env::Environment;
use crate::error::EvalError;
use crate::eval::evaluate;
use crate::value::Value;

/// `Decimal` 最多可表示的小數位數。
const MAX_SCALE: i16 = 28;

/// 把計算結果依公式欄位的 `formula_result_type` 轉成 JSON 存值。
///
/// | 結果型態 | 接受的值 | JSON |
/// |---|---|---|
/// | `integer` | 數字，四捨五入到整數，範圍 ±2^53 − 1 | number |
/// | `decimal`、`money` | 數字，依 `decimal_places` 四捨五入並補足小數位數 | string，例如 `"12.3400"` |
/// | `text`、`long_text` | 文字 | string |
/// | `boolean` | 布林 | boolean |
/// | `date` | 日期 | string `YYYY-MM-DD` |
/// | `datetime` | 日期時間 | string RFC 3339 |
///
/// 空值一律是 JSON `null`。四捨五入使用 MidpointAwayFromZero。
/// `decimal_places` 只在 `decimal`、`money` 使用，必須介於 0 到 28。
pub fn to_storage(
    value: &Value,
    result_type: DataType,
    decimal_places: Option<i16>,
) -> Result<Json, EvalError> {
    let mismatch = || EvalError::ResultTypeMismatch {
        expected: result_type,
        found: value.kind(),
    };
    let field_value = match (result_type, value) {
        (
            DataType::Select
            | DataType::MultiSelect
            | DataType::Relation
            | DataType::File
            | DataType::Formula,
            _,
        ) => return Err(EvalError::UnsupportedResultType(result_type)),
        (_, Value::Null) => return Ok(Json::Null),
        (DataType::Integer, Value::Number(n)) => {
            let rounded = n.round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero);
            let int = rounded
                .to_i64()
                .filter(|i| i.unsigned_abs() <= MAX_SAFE_INTEGER.unsigned_abs())
                .ok_or(EvalError::Overflow)?;
            FieldValue::Integer(int)
        }
        (DataType::Decimal | DataType::Money, Value::Number(n)) => {
            FieldValue::Decimal(round_to_places(*n, decimal_places)?)
        }
        (DataType::Text | DataType::LongText, Value::Text(s)) => FieldValue::Text(s.clone()),
        (DataType::Boolean, Value::Bool(b)) => FieldValue::Boolean(*b),
        (DataType::Date, Value::Date(d)) => FieldValue::Date(*d),
        (DataType::DateTime, Value::DateTime(dt)) => FieldValue::DateTime(*dt),
        _ => return Err(mismatch()),
    };
    Ok(to_json(&field_value))
}

/// 四捨五入到 `places` 位，並補足小數位數（`12.3` → `12.3000`）。
fn round_to_places(n: Decimal, places: Option<i16>) -> Result<Decimal, EvalError> {
    let p = places
        .filter(|p| (0..=MAX_SCALE).contains(p))
        .ok_or(EvalError::InvalidDecimalPlaces(places))?;
    let p = u32::from(p.unsigned_abs());
    let mut rounded = n.round_dp_with_strategy(p, RoundingStrategy::MidpointAwayFromZero);
    // 整數位數太多時補不了小數位數（Decimal 最多 28–29 位有效數字）
    if rounded.scale() < p {
        let int_digits = rounded
            .trunc()
            .abs()
            .to_string()
            .trim_start_matches('0')
            .len();
        if int_digits + p as usize > MAX_SCALE as usize {
            return Err(EvalError::Overflow);
        }
        rounded.rescale(p);
    }
    Ok(rounded)
}

/// 公式欄位的計算結果：存入資料的 JSON 值與錯誤原因。
#[derive(Debug, Clone, PartialEq)]
pub struct Computed {
    /// §7.3 的 JSON 存值；計算失敗時為 `null`。
    pub value: Json,
    /// 失敗原因；成功時為 `None`。
    pub error: Option<EvalError>,
}

/// 計算公式並轉成存值：[`evaluate`] 加上 [`to_storage`]。失敗時值為 `null` 並帶錯誤原因。
pub fn compute(
    expr: &Expr,
    env: &dyn Environment,
    result_type: DataType,
    decimal_places: Option<i16>,
) -> Computed {
    match evaluate(expr, env).and_then(|v| to_storage(&v, result_type, decimal_places)) {
        Ok(value) => Computed { value, error: None },
        Err(e) => Computed {
            value: Json::Null,
            error: Some(e),
        },
    }
}
