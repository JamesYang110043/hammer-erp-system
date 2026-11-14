//! 欄位資料型態變更（docs/architecture.md §7.3、§7.4，純邏輯，不存取資料庫）。
//!
//! - [`classify`]：判斷型態轉換是否允許，以及是否一定安全
//! - [`convert_value`]：轉換一個值，並依新的欄位定義正規化
//! - [`convert_records`]：轉換一個資料類別的所有資料，收集所有失敗
//!
//! # 規則（§7.4）
//!
//! | 從 → 到 | 規則 |
//! |---|---|
//! | 任意型態 → `text` / `long_text` | 允許（[`TypeChange::Safe`]） |
//! | `text` ↔ `long_text` | 允許（[`TypeChange::Safe`]） |
//! | `integer` → `decimal` | 允許（[`TypeChange::Safe`]） |
//! | `select` → `multi_select` | 允許（[`TypeChange::Safe`]） |
//! | 其他 | 逐筆轉換（[`TypeChange::Checked`]），只要有一筆失敗就拒絕 |
//!
//! `relation` 只能轉成 `text` / `long_text`（值為資料 id 的文字），其他型態不能轉成 `relation`；
//! `file`、`formula` 不能轉換（M2b、M2c）。
//!
//! 「允許」代表型態本身一定能轉換。轉換後的值一律再依新的欄位定義正規化
//! （[`normalize_field_value`]），所以新的設定比較嚴格時（例如字元數上限、選項、小數位數），
//! 安全的轉換也可能失敗。
//!
//! # 各型態的轉換方式
//!
//! - 轉成 `text` / `long_text`：文字原樣保留；數字、日期、日期時間使用 §7.3 的存值格式；
//!   勾選為 `true` / `false`；多選以 `, ` 連接選項值
//! - 由文字轉成其他型態：先去除前後空白，只有空白時視為沒有值
//! - `integer`：文字須為整數（例如 `"12"`）；`decimal` / `money` 的小數部分必須為 0
//! - `decimal` / `money`：文字須符合 §7.3 的數字格式；`integer` 直接轉換；小數位數不可超過新的設定
//! - `date`：文字須為 `YYYY-MM-DD`；`datetime` 取台北時間的日期（D13）
//! - `datetime`：文字須為 RFC 3339；`date` 轉成台北時間當天 00:00
//! - `boolean`：文字須為 `true` / `false`（不分大小寫）
//! - `select`：多選只有一個選項時取該選項；其他型態以文字比對選項值
//! - `multi_select`：變成只有一個選項的陣列
//!
//! 沒有值的欄位不轉換。

use std::collections::HashMap;

use hammer_shared::Id;
use hammer_shared::schema::{DataType, FieldDefinition, FieldValue, parse_value, to_json};
use hammer_shared::time::TAIPEI_OFFSET;
use serde_json::{Map, Value};

use crate::record_data::normalize_field_value;

/// 轉換後與其他資料重複時的訊息。
pub const MSG_DUPLICATE: &str = "轉換後的值與其他資料重複";
/// 資料庫中原本的值不符合原型態的存值格式時的訊息。
pub const MSG_CORRUPTED: &str = "原本的值格式不正確";

/// 型態轉換的種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeChange {
    /// 型態本身一定能轉換（§7.4 直接允許）。
    Safe,
    /// 須逐筆轉換，有一筆失敗就拒絕。
    Checked,
}

/// 判斷 `from` → `to` 是否允許。不允許時回傳顯示給使用者的訊息。
pub fn classify(from: DataType, to: DataType) -> Result<TypeChange, &'static str> {
    if from == to {
        return Err("新的資料型態與目前相同");
    }
    if to == DataType::Relation {
        return Err("其他型態不能轉換成關聯");
    }
    if from == DataType::Relation && !matches!(to, DataType::Text | DataType::LongText) {
        return Err("關聯欄位只能轉換成單行文字或多行文字");
    }
    if !from.is_value_supported() || !to.is_value_supported() {
        return Err("附件、公式欄位目前不能變更資料型態");
    }
    let safe = matches!(to, DataType::Text | DataType::LongText)
        || matches!(
            (from, to),
            (DataType::Integer, DataType::Decimal) | (DataType::Select, DataType::MultiSelect)
        );
    Ok(if safe {
        TypeChange::Safe
    } else {
        TypeChange::Checked
    })
}

/// 把 `from` 欄位的存值 `json` 轉成 `to` 欄位的存值，並依 `to` 的設定正規化。
///
/// 沒有值時回傳 `Ok(None)`。錯誤為顯示給使用者的說明。
pub fn convert_value(
    from: &FieldDefinition,
    to: &FieldDefinition,
    json: &Value,
) -> Result<Option<Value>, String> {
    let Some(value) = parse_value(from.data_type, json).map_err(|_| MSG_CORRUPTED.to_owned())?
    else {
        return Ok(None);
    };
    let Some(candidate) = candidate(value, to.data_type)? else {
        return Ok(None);
    };
    normalize_field_value(to, &candidate)
}

/// 依目標型態產生存值格式的候選值（尚未依欄位設定檢查）。
fn candidate(value: FieldValue, to: DataType) -> Result<Option<Value>, String> {
    let cannot = || Err(format!("無法轉換成{}", type_label(to)));

    // 由文字轉成其他型態：去除前後空白，只有空白時視為沒有值
    let value = match value {
        FieldValue::Text(s) if !matches!(to, DataType::Text | DataType::LongText) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            FieldValue::Text(trimmed.to_owned())
        }
        other => other,
    };

    let json = match to {
        DataType::Text | DataType::LongText => Value::String(text_of(&value)),
        DataType::Integer => match value {
            FieldValue::Integer(n) => Value::from(n),
            FieldValue::Decimal(d) => {
                if !d.fract().is_zero() {
                    return Err("有小數，無法轉換成整數".to_owned());
                }
                let n = i64::try_from(d.trunc()).map_err(|_| "數字超出整數範圍".to_owned())?;
                Value::from(n)
            }
            FieldValue::Text(s) => {
                let n = s.parse::<i64>().map_err(|_| format!("「{s}」不是整數"))?;
                Value::from(n)
            }
            _ => return cannot(),
        },
        DataType::Decimal | DataType::Money => match value {
            FieldValue::Integer(n) => Value::String(n.to_string()),
            FieldValue::Decimal(d) => Value::String(d.to_string()),
            FieldValue::Text(s) => Value::String(s),
            _ => return cannot(),
        },
        DataType::Date => match value {
            FieldValue::Text(s) => Value::String(s),
            FieldValue::DateTime(dt) => {
                to_json(&FieldValue::Date(dt.to_offset(TAIPEI_OFFSET).date()))
            }
            _ => return cannot(),
        },
        DataType::DateTime => match value {
            FieldValue::Text(s) => Value::String(s),
            FieldValue::Date(d) => to_json(&FieldValue::DateTime(
                d.midnight().assume_offset(TAIPEI_OFFSET),
            )),
            _ => return cannot(),
        },
        DataType::Boolean => match value {
            FieldValue::Boolean(b) => Value::Bool(b),
            FieldValue::Text(s) => match s.to_ascii_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(format!("「{s}」不是 true 或 false")),
            },
            _ => return cannot(),
        },
        DataType::Select => match value {
            FieldValue::MultiSelect(mut items) => match items.len() {
                0 => return Ok(None),
                1 => Value::String(items.remove(0)),
                _ => return Err("有多個選項，無法轉換成單選".to_owned()),
            },
            other => Value::String(text_of(&other)),
        },
        DataType::MultiSelect => match value {
            FieldValue::MultiSelect(items) => {
                Value::Array(items.into_iter().map(Value::String).collect())
            }
            other => Value::Array(vec![Value::String(text_of(&other))]),
        },
        DataType::Relation | DataType::File | DataType::Formula => return cannot(),
    };
    Ok(Some(json))
}

/// 值的文字表示（轉成 `text` / `long_text` 使用）。
fn text_of(value: &FieldValue) -> String {
    match value {
        FieldValue::Text(s) => s.clone(),
        FieldValue::Integer(n) => n.to_string(),
        FieldValue::Boolean(b) => b.to_string(),
        FieldValue::MultiSelect(items) => items.join(", "),
        FieldValue::Decimal(_)
        | FieldValue::Date(_)
        | FieldValue::DateTime(_)
        | FieldValue::Relation(_) => match to_json(value) {
            Value::String(s) => s,
            other => other.to_string(),
        },
    }
}

/// 顯示給使用者的型態名稱。
pub fn type_label(t: DataType) -> &'static str {
    match t {
        DataType::Text => "單行文字",
        DataType::LongText => "多行文字",
        DataType::Integer => "整數",
        DataType::Decimal => "小數",
        DataType::Money => "金額",
        DataType::Date => "日期",
        DataType::DateTime => "日期時間",
        DataType::Boolean => "勾選",
        DataType::Select => "單選",
        DataType::MultiSelect => "多選",
        DataType::Relation => "關聯",
        DataType::File => "附件",
        DataType::Formula => "公式",
    }
}

/// 要轉換的一筆資料。
#[derive(Debug, Clone, Copy)]
pub struct StoredRecord<'a> {
    pub id: Id,
    pub data: &'a Map<String, Value>,
    /// 已軟刪除。
    pub deleted: bool,
}

/// 轉換後內容有變動的一筆資料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedRecord {
    pub id: Id,
    /// 轉換後的完整 `data`。
    pub data: Map<String, Value>,
}

/// 一筆無法轉換的資料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionFailure {
    pub record_id: Id,
    /// 原本的值。
    pub value: Value,
    /// 已軟刪除。
    pub deleted: bool,
    /// 顯示給使用者的說明。
    pub message: String,
}

/// 轉換所有資料的 `from.field_key` 欄位。
///
/// - 全部成功：回傳內容有變動的資料（沒有值、轉換後相同的資料不列入）
/// - 有失敗：回傳所有失敗，依 `records` 的順序
///
/// `to.is_unique` 時，未刪除的資料轉換後的值不可重複（§7.5 的唯一索引只涵蓋未刪除的資料），
/// 重複的每一筆都列為失敗。
pub fn convert_records(
    from: &FieldDefinition,
    to: &FieldDefinition,
    records: &[StoredRecord<'_>],
) -> Result<Vec<ConvertedRecord>, Vec<ConversionFailure>> {
    let key = from.field_key.as_str();
    let mut failures: Vec<(usize, ConversionFailure)> = Vec::new();
    let mut converted = Vec::new();
    // 轉換後的值（序列化後的字串）→ 未刪除資料的索引
    let mut seen: HashMap<String, Vec<usize>> = HashMap::new();

    for (index, record) in records.iter().enumerate() {
        let Some(original) = record.data.get(key) else {
            continue;
        };
        let value = match convert_value(from, to, original) {
            Ok(v) => v,
            Err(message) => {
                failures.push((index, failure(record, original, message)));
                continue;
            }
        };
        if to.is_unique
            && !record.deleted
            && let Some(v) = value.as_ref()
        {
            seen.entry(v.to_string()).or_default().push(index);
        }
        if value.as_ref() == Some(original) {
            continue;
        }
        let mut data = record.data.clone();
        match value {
            Some(v) => data.insert(key.to_owned(), v),
            None => data.remove(key),
        };
        converted.push(ConvertedRecord {
            id: record.id,
            data,
        });
    }

    for indexes in seen.values().filter(|i| i.len() > 1) {
        for &index in indexes {
            let record = &records[index];
            if let Some(original) = record.data.get(key) {
                failures.push((index, failure(record, original, MSG_DUPLICATE.to_owned())));
            }
        }
    }

    if failures.is_empty() {
        Ok(converted)
    } else {
        failures.sort_by_key(|(index, _)| *index);
        Err(failures.into_iter().map(|(_, f)| f).collect())
    }
}

fn failure(record: &StoredRecord<'_>, original: &Value, message: String) -> ConversionFailure {
    ConversionFailure {
        record_id: record.id,
        value: original.clone(),
        deleted: record.deleted,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hammer_shared::new_id;
    use hammer_shared::schema::{FieldStorage, SelectOption, Validation};
    use serde_json::json;
    use time::macros::datetime;

    const SUPPORTED: &[DataType] = &[
        DataType::Text,
        DataType::LongText,
        DataType::Integer,
        DataType::Decimal,
        DataType::Money,
        DataType::Date,
        DataType::DateTime,
        DataType::Boolean,
        DataType::Select,
        DataType::MultiSelect,
    ];

    /// 寬鬆的欄位設定：小數 4 位、選項包含測試會用到的值。
    fn field(data_type: DataType) -> FieldDefinition {
        let now = datetime!(2026-11-13 09:00 +8);
        FieldDefinition {
            id: new_id(),
            entity_id: new_id(),
            field_key: "f".into(),
            label: "欄位".into(),
            data_type,
            is_system: false,
            storage: FieldStorage::Json,
            is_required: false,
            is_visible: true,
            is_readonly: false,
            is_searchable: false,
            is_unique: false,
            default_value: None,
            validation: None,
            options: data_type.uses_options().then(|| {
                ["a", "b", "12", "true", "2026-11-13"]
                    .iter()
                    .map(|v| SelectOption {
                        value: (*v).into(),
                        label: v.to_uppercase(),
                        color: None,
                    })
                    .collect()
            }),
            decimal_places: data_type.uses_decimal_places().then_some(4),
            relation_entity_id: None,
            formula: None,
            formula_result_type: None,
            ui_widget: None,
            column_width: None,
            help_text: None,
            sort_order: 0,
            is_active: true,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    /// 各型態的代表值（存值格式）。
    fn samples(t: DataType) -> Vec<Value> {
        match t {
            DataType::Text | DataType::LongText => vec![json!("a"), json!("第一行\n第二行")],
            DataType::Integer => vec![json!(12), json!(-3), json!(0)],
            DataType::Decimal | DataType::Money => vec![json!("12.3400"), json!("-0.5000")],
            DataType::Date => vec![json!("2026-11-13")],
            DataType::DateTime => vec![json!("2026-11-13T09:30:00+08:00")],
            DataType::Boolean => vec![json!(true), json!(false)],
            DataType::Select => vec![json!("a"), json!("12")],
            DataType::MultiSelect => vec![json!(["a"]), json!(["a", "b"])],
            _ => vec![],
        }
    }

    fn convert(from: DataType, to: DataType, value: Value) -> Result<Option<Value>, String> {
        convert_value(&field(from), &field(to), &value)
    }

    fn ok(from: DataType, to: DataType, value: Value) -> Value {
        convert(from, to, value.clone())
            .unwrap_or_else(|e| panic!("{from:?} → {to:?} {value} 應該成功：{e}"))
            .unwrap()
    }

    fn fails(from: DataType, to: DataType, value: Value) -> String {
        convert(from, to, value.clone()).expect_err(&format!("{from:?} → {to:?} {value} 應該失敗"))
    }

    // -----------------------------------------------------------------------
    // §7.4 表格
    // -----------------------------------------------------------------------

    #[test]
    fn classify_follows_the_table() {
        use DataType::*;
        for &from in SUPPORTED {
            for &to in SUPPORTED {
                let result = classify(from, to);
                if from == to {
                    assert!(result.is_err(), "{from:?} → {to:?}");
                    continue;
                }
                let expected = if matches!(to, Text | LongText)
                    || (from, to) == (Integer, Decimal)
                    || (from, to) == (Select, MultiSelect)
                {
                    TypeChange::Safe
                } else {
                    TypeChange::Checked
                };
                assert_eq!(result, Ok(expected), "{from:?} → {to:?}");
            }
        }
        assert_eq!(classify(Text, LongText), Ok(TypeChange::Safe));
        assert_eq!(classify(LongText, Text), Ok(TypeChange::Safe));
        assert_eq!(classify(Decimal, Integer), Ok(TypeChange::Checked));
        assert_eq!(classify(MultiSelect, Select), Ok(TypeChange::Checked));
        assert_eq!(classify(Integer, Money), Ok(TypeChange::Checked));
    }

    #[test]
    fn file_formula_cannot_change() {
        for &t in SUPPORTED {
            for other in [DataType::File, DataType::Formula] {
                assert!(classify(t, other).is_err(), "{t:?} → {other:?}");
                assert!(classify(other, t).is_err(), "{other:?} → {t:?}");
            }
        }
    }

    /// `relation` 只能轉成 text / long_text；其他型態不能轉成 `relation`（§7.4）。
    #[test]
    fn relation_only_changes_to_text() {
        use DataType::*;
        for &t in SUPPORTED {
            assert!(classify(t, Relation).is_err(), "{t:?} → Relation");
            let expected_ok = matches!(t, Text | LongText);
            assert_eq!(
                classify(Relation, t).is_ok(),
                expected_ok,
                "Relation → {t:?}"
            );
        }
        for other in [File, Formula] {
            assert!(classify(Relation, other).is_err());
            assert!(classify(other, Relation).is_err());
        }
        assert_eq!(classify(Relation, Text), Ok(TypeChange::Safe));
        assert_eq!(classify(Relation, LongText), Ok(TypeChange::Safe));

        let id = "0190c3a2-0000-7000-8000-000000000000";
        assert_eq!(ok(Relation, Text, json!(id)), json!(id));
        assert_eq!(ok(Relation, LongText, json!(id)), json!(id));
        assert_eq!(convert(Relation, Text, Value::Null), Ok(None));
    }

    /// 任意型態 → text / long_text：一定成功。
    #[test]
    fn any_type_to_text() {
        for &from in SUPPORTED {
            for to in [DataType::Text, DataType::LongText] {
                if from == to {
                    continue;
                }
                for v in samples(from) {
                    assert!(ok(from, to, v).is_string());
                }
            }
        }
        assert_eq!(
            ok(DataType::Integer, DataType::Text, json!(12)),
            json!("12")
        );
        assert_eq!(
            ok(DataType::Decimal, DataType::Text, json!("12.3400")),
            json!("12.3400")
        );
        assert_eq!(
            ok(DataType::Boolean, DataType::Text, json!(true)),
            json!("true")
        );
        assert_eq!(
            ok(DataType::Date, DataType::LongText, json!("2026-11-13")),
            json!("2026-11-13")
        );
        assert_eq!(
            ok(
                DataType::DateTime,
                DataType::Text,
                json!("2026-11-13T09:30:00+08:00")
            ),
            json!("2026-11-13T09:30:00+08:00")
        );
        assert_eq!(ok(DataType::Select, DataType::Text, json!("a")), json!("a"));
        assert_eq!(
            ok(DataType::MultiSelect, DataType::Text, json!(["a", "b"])),
            json!("a, b")
        );
    }

    /// text ↔ long_text：原樣保留，包含換行與前後空白。
    #[test]
    fn text_and_long_text() {
        assert_eq!(
            ok(DataType::LongText, DataType::Text, json!("第一行\n第二行")),
            json!("第一行\n第二行")
        );
        assert_eq!(
            ok(DataType::Text, DataType::LongText, json!(" a ")),
            json!(" a ")
        );
    }

    /// integer → decimal：補零到小數位數。
    #[test]
    fn integer_to_decimal() {
        for v in samples(DataType::Integer) {
            ok(DataType::Integer, DataType::Decimal, v);
        }
        assert_eq!(
            ok(DataType::Integer, DataType::Decimal, json!(12)),
            json!("12.0000")
        );
        let to = FieldDefinition {
            decimal_places: Some(0),
            ..field(DataType::Decimal)
        };
        assert_eq!(
            convert_value(&field(DataType::Integer), &to, &json!(-3)),
            Ok(Some(json!("-3")))
        );
    }

    /// select → multi_select：變成只有一個選項的陣列。
    #[test]
    fn select_to_multi_select() {
        assert_eq!(
            ok(DataType::Select, DataType::MultiSelect, json!("a")),
            json!(["a"])
        );
    }

    /// 安全的轉換仍須符合新的設定。
    #[test]
    fn safe_conversion_still_checks_new_settings() {
        let to = FieldDefinition {
            validation: Some(Validation {
                max_length: Some(3),
                ..Default::default()
            }),
            ..field(DataType::Text)
        };
        assert!(convert_value(&field(DataType::Decimal), &to, &json!("12.3400")).is_err());

        let to = FieldDefinition {
            options: Some(vec![SelectOption {
                value: "b".into(),
                label: "B".into(),
                color: None,
            }]),
            ..field(DataType::MultiSelect)
        };
        assert!(convert_value(&field(DataType::Select), &to, &json!("a")).is_err());
    }

    // -----------------------------------------------------------------------
    // 其他轉換：逐筆檢查
    // -----------------------------------------------------------------------

    #[test]
    fn text_to_integer() {
        assert_eq!(
            ok(DataType::Text, DataType::Integer, json!("12")),
            json!(12)
        );
        assert_eq!(
            ok(DataType::Text, DataType::Integer, json!(" -7 ")),
            json!(-7)
        );
        assert!(fails(DataType::Text, DataType::Integer, json!("abc")).contains("abc"));
        fails(DataType::Text, DataType::Integer, json!("12.5"));
        fails(DataType::Text, DataType::Integer, json!("1e3"));
        // 超出 ±2^53 − 1
        fails(DataType::Text, DataType::Integer, json!("9007199254740992"));
        // 只有空白：沒有值
        assert_eq!(
            convert(DataType::Text, DataType::Integer, json!("  ")),
            Ok(None)
        );
        assert_eq!(
            ok(DataType::LongText, DataType::Integer, json!("100")),
            json!(100)
        );
    }

    #[test]
    fn decimal_and_money_to_integer() {
        for from in [DataType::Decimal, DataType::Money] {
            assert_eq!(ok(from, DataType::Integer, json!("12.0000")), json!(12));
            assert_eq!(ok(from, DataType::Integer, json!("-3")), json!(-3));
            fails(from, DataType::Integer, json!("12.5000"));
        }
    }

    #[test]
    fn to_decimal_and_money() {
        assert_eq!(
            ok(DataType::Text, DataType::Decimal, json!("12.5")),
            json!("12.5000")
        );
        fails(DataType::Text, DataType::Decimal, json!("12,000"));
        fails(DataType::Text, DataType::Decimal, json!("abc"));
        assert_eq!(
            ok(DataType::Integer, DataType::Money, json!(7)),
            json!("7.0000")
        );
        assert_eq!(
            ok(DataType::Decimal, DataType::Money, json!("1.2300")),
            json!("1.2300")
        );
        assert_eq!(
            ok(DataType::Money, DataType::Decimal, json!("1.2300")),
            json!("1.2300")
        );
        // 小數位數不可超過新的設定，不自動四捨五入
        let to = FieldDefinition {
            decimal_places: Some(1),
            ..field(DataType::Decimal)
        };
        let from = field(DataType::Decimal);
        assert_eq!(
            convert_value(&from, &to, &json!("1.2000")),
            Ok(Some(json!("1.2")))
        );
        assert!(convert_value(&from, &to, &json!("1.2300")).is_err());
        fails(DataType::Boolean, DataType::Decimal, json!(true));
        fails(DataType::Date, DataType::Money, json!("2026-11-13"));
    }

    #[test]
    fn to_date_and_datetime() {
        assert_eq!(
            ok(DataType::Text, DataType::Date, json!("2026-11-13")),
            json!("2026-11-13")
        );
        fails(DataType::Text, DataType::Date, json!("2026/11/13"));
        fails(DataType::Text, DataType::Date, json!("2026-02-30"));
        // datetime → date 取台北時間的日期
        assert_eq!(
            ok(
                DataType::DateTime,
                DataType::Date,
                json!("2026-11-13T20:00:00Z")
            ),
            json!("2026-11-14")
        );
        assert_eq!(
            ok(DataType::Date, DataType::DateTime, json!("2026-11-13")),
            json!("2026-11-13T00:00:00+08:00")
        );
        assert_eq!(
            ok(
                DataType::Text,
                DataType::DateTime,
                json!("2026-11-13T09:30:00Z")
            ),
            json!("2026-11-13T09:30:00Z")
        );
        fails(DataType::Text, DataType::DateTime, json!("2026-11-13"));
        fails(DataType::Integer, DataType::Date, json!(20261113));
    }

    #[test]
    fn to_boolean() {
        assert_eq!(
            ok(DataType::Text, DataType::Boolean, json!("true")),
            json!(true)
        );
        assert_eq!(
            ok(DataType::Text, DataType::Boolean, json!("FALSE")),
            json!(false)
        );
        fails(DataType::Text, DataType::Boolean, json!("是"));
        fails(DataType::Integer, DataType::Boolean, json!(1));
        assert_eq!(
            ok(DataType::Select, DataType::Boolean, json!("true")),
            json!(true)
        );
    }

    #[test]
    fn boolean_and_dates_to_numbers_fail() {
        fails(DataType::Boolean, DataType::Integer, json!(true));
        fails(DataType::Date, DataType::Integer, json!("2026-11-13"));
        fails(
            DataType::DateTime,
            DataType::Decimal,
            json!("2026-11-13T09:30:00Z"),
        );
        fails(DataType::Boolean, DataType::Date, json!(false));
    }

    #[test]
    fn to_select() {
        assert_eq!(
            ok(DataType::Text, DataType::Select, json!(" a ")),
            json!("a")
        );
        assert!(fails(DataType::Text, DataType::Select, json!("z")).contains('z'));
        assert_eq!(
            ok(DataType::Integer, DataType::Select, json!(12)),
            json!("12")
        );
        assert_eq!(
            ok(DataType::MultiSelect, DataType::Select, json!(["b"])),
            json!("b")
        );
        fails(DataType::MultiSelect, DataType::Select, json!(["a", "b"]));
    }

    #[test]
    fn to_multi_select() {
        assert_eq!(
            ok(DataType::Text, DataType::MultiSelect, json!("b")),
            json!(["b"])
        );
        fails(DataType::Text, DataType::MultiSelect, json!("z"));
        assert_eq!(
            ok(DataType::Boolean, DataType::MultiSelect, json!(true)),
            json!(["true"])
        );
    }

    #[test]
    fn null_and_corrupted_values() {
        assert_eq!(
            convert(DataType::Text, DataType::Integer, Value::Null),
            Ok(None)
        );
        // 資料庫中原本的值不符合原型態
        assert_eq!(
            fails(DataType::Integer, DataType::Decimal, json!("12")),
            MSG_CORRUPTED
        );
    }

    // -----------------------------------------------------------------------
    // convert_records
    // -----------------------------------------------------------------------

    fn data(value: Option<Value>) -> Map<String, Value> {
        let mut map = Map::new();
        map.insert("other".into(), json!("x"));
        if let Some(v) = value {
            map.insert("f".into(), v);
        }
        map
    }

    #[test]
    fn convert_records_returns_only_changed_records() {
        let datas = [
            data(Some(json!("12"))),
            data(None),
            data(Some(json!("  "))),
            data(Some(json!("7"))),
        ];
        let ids: Vec<Id> = datas.iter().map(|_| new_id()).collect();
        let records: Vec<_> = datas
            .iter()
            .zip(&ids)
            .map(|(d, id)| StoredRecord {
                id: *id,
                data: d,
                deleted: false,
            })
            .collect();
        let converted =
            convert_records(&field(DataType::Text), &field(DataType::Integer), &records).unwrap();
        assert_eq!(converted.len(), 3);
        assert_eq!(converted[0].id, ids[0]);
        assert_eq!(converted[0].data["f"], json!(12));
        assert_eq!(converted[0].data["other"], json!("x"));
        // 只有空白：移除該欄位
        assert_eq!(converted[1].id, ids[2]);
        assert!(!converted[1].data.contains_key("f"));
        assert_eq!(converted[2].data["f"], json!(7));

        // 轉換後相同的資料不列入
        let text = data(Some(json!("a")));
        let records = [StoredRecord {
            id: new_id(),
            data: &text,
            deleted: false,
        }];
        assert_eq!(
            convert_records(&field(DataType::Text), &field(DataType::LongText), &records),
            Ok(vec![])
        );
    }

    #[test]
    fn convert_records_collects_all_failures_including_deleted() {
        let datas = [
            data(Some(json!("abc"))),
            data(Some(json!("12"))),
            data(Some(json!("x"))),
        ];
        let records: Vec<_> = datas
            .iter()
            .enumerate()
            .map(|(i, d)| StoredRecord {
                id: new_id(),
                data: d,
                deleted: i == 2,
            })
            .collect();
        let failures = convert_records(&field(DataType::Text), &field(DataType::Integer), &records)
            .unwrap_err();
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].record_id, records[0].id);
        assert_eq!(failures[0].value, json!("abc"));
        assert!(!failures[0].deleted);
        assert_eq!(failures[1].record_id, records[2].id);
        assert!(failures[1].deleted);
    }

    #[test]
    fn convert_records_checks_uniqueness_among_active_records() {
        let to = FieldDefinition {
            is_unique: true,
            ..field(DataType::Integer)
        };
        let datas = [
            data(Some(json!("12"))),
            data(Some(json!("7"))),
            data(Some(json!(" 12"))),
            data(Some(json!("7"))),
        ];
        // 第 4 筆已刪除，不與第 2 筆衝突
        let records: Vec<_> = datas
            .iter()
            .enumerate()
            .map(|(i, d)| StoredRecord {
                id: new_id(),
                data: d,
                deleted: i == 3,
            })
            .collect();
        let failures = convert_records(&field(DataType::Text), &to, &records).unwrap_err();
        let ids: Vec<_> = failures.iter().map(|f| f.record_id).collect();
        assert_eq!(ids, [records[0].id, records[2].id]);
        assert!(failures.iter().all(|f| f.message == MSG_DUPLICATE));
        assert_eq!(failures[1].value, json!(" 12"));
    }
}
