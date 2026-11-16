//! 計算環境：由呼叫端提供欄位值與「今天」的日期。

use hammer_shared::schema::{DataType, FieldValue, parse_value};
use hammer_shared::time::Date;
use serde_json::Value as Json;

use crate::error::EvalError;
use crate::value::Value;

static JSON_NULL: Json = Json::Null;

/// 一個欄位的值：欄位型態與 §7.3 的 JSON 存值。
///
/// `formula` 欄位的 `data_type` 由呼叫端傳入它的 `formula_result_type`（例如 `decimal`），
/// 不要傳 [`DataType::Formula`]。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldData<'a> {
    pub data_type: DataType,
    pub json: &'a Json,
}

impl<'a> FieldData<'a> {
    pub fn new(data_type: DataType, json: &'a Json) -> Self {
        Self { data_type, json }
    }

    /// 沒有值的欄位（JSON `null`）。
    pub fn null(data_type: DataType) -> FieldData<'static> {
        FieldData {
            data_type,
            json: &JSON_NULL,
        }
    }
}

/// 明細子表某個欄位在所有列的值，依列的順序。
#[derive(Debug, Clone, PartialEq)]
pub struct DetailColumn<'a> {
    pub data_type: DataType,
    pub values: Vec<&'a Json>,
}

/// 計算環境。伺服器由 `crates/core` 從資料庫讀取資料實作；桌面端以表單上的值實作。
///
/// 回傳 `None` 表示**欄位不存在**，計算會失敗（[`EvalError::UnknownField`] 等）；
/// 欄位存在但沒有值時回傳 JSON `null`（可用 [`FieldData::null`]）。
pub trait Environment {
    /// 「今天」的日期。伺服器使用 Asia/Taipei（D13，`hammer_shared::time::today_taipei`）。
    fn today(&self) -> Date;

    /// 本筆資料的欄位 `{field_key}`。
    fn field(&self, key: &str) -> Option<FieldData<'_>>;

    /// 關聯引用 `{relation.field}`：`relation` 欄位所指資料的 `field` 欄位。
    ///
    /// 關聯欄位沒有值時，回傳目標欄位型態的 `null`。
    fn relation_field(&self, relation: &str, field: &str) -> Option<FieldData<'_>>;

    /// 明細子表 `detail` 所有列的 `field` 欄位。沒有明細時 `values` 為空。
    fn detail_column(&self, detail: &str, field: &str) -> Option<DetailColumn<'_>>;
}

/// 依欄位型態把 JSON 存值轉成 [`Value`]。
///
/// `field` 是錯誤訊息中顯示的欄位，例如 `qty`、`customer.discount_rate`、`lines.amount`。
///
/// | data_type | 值 |
/// |---|---|
/// | `integer`、`decimal`、`money` | 數字 |
/// | `text`、`long_text`、`select` | 文字 |
/// | `boolean` | 布林 |
/// | `date` | 日期 |
/// | `datetime` | 日期時間 |
/// | `multi_select`、`relation`、`file`、`formula` | 不支援（[`EvalError::UnsupportedFieldType`]） |
pub fn json_to_value(field: &str, data: FieldData<'_>) -> Result<Value, EvalError> {
    if matches!(
        data.data_type,
        DataType::MultiSelect | DataType::Relation | DataType::File | DataType::Formula
    ) {
        return Err(EvalError::UnsupportedFieldType {
            field: field.to_owned(),
            data_type: data.data_type,
        });
    }
    let parsed =
        parse_value(data.data_type, data.json).map_err(|reason| EvalError::InvalidStoredValue {
            field: field.to_owned(),
            reason,
        })?;
    Ok(match parsed {
        None => Value::Null,
        Some(FieldValue::Integer(n)) => Value::Number(n.into()),
        Some(FieldValue::Decimal(d)) => Value::Number(d),
        Some(FieldValue::Text(s)) => Value::Text(s),
        Some(FieldValue::Boolean(b)) => Value::Bool(b),
        Some(FieldValue::Date(d)) => Value::Date(d),
        Some(FieldValue::DateTime(dt)) => Value::DateTime(dt),
        // 上面已排除這些型態，parse_value 不會回傳
        Some(FieldValue::MultiSelect(_) | FieldValue::Relation(_) | FieldValue::File(_)) => {
            return Err(EvalError::UnsupportedFieldType {
                field: field.to_owned(),
                data_type: data.data_type,
            });
        }
    })
}
