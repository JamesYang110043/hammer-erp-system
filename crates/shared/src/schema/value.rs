//! 欄位值的 JSON 存值格式（`docs/architecture.md` §7.3、D9）。
//!
//! `custom_records.data`、`custom_data` 以及 API 傳遞的欄位值都使用以下格式：
//!
//! | data_type | JSON | [`FieldValue`] |
//! |---|---|---|
//! | `text`、`long_text`、`select` | string | [`FieldValue::Text`] |
//! | `integer` | number（整數，範圍為 ±[`MAX_SAFE_INTEGER`]） | [`FieldValue::Integer`] |
//! | `decimal`、`money` | **string**，例如 `"12.3400"` | [`FieldValue::Decimal`] |
//! | `date` | string `YYYY-MM-DD` | [`FieldValue::Date`] |
//! | `datetime` | string RFC 3339 | [`FieldValue::DateTime`] |
//! | `boolean` | boolean | [`FieldValue::Boolean`] |
//! | `multi_select` | string[]（不可重複） | [`FieldValue::MultiSelect`] |
//! | `relation` | string，目標資料的 id（UUID），存成小寫連字號格式 | [`FieldValue::Relation`] |
//! | `file` | string，附件的 id（`attachments.id`，UUID），存成小寫連字號格式 | [`FieldValue::File`] |
//!
//! JSON `null` 代表沒有值（[`parse_value`] 回傳 `Ok(None)`）。
//!
//! `formula` 的值在 M2c 處理，目前回傳 [`InvalidValue::Unsupported`]。
//!
//! 這裡只檢查「格式」。必填、`validation`、選項是否存在、小數位數、關聯的資料是否存在等
//! 依欄位定義的檢查不在此處；附件是否存在、是否已被使用在 `hammer_core::attachments`。

use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;
use time::format_description::BorrowedFormatItem;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Date, OffsetDateTime};

use super::DataType;
use crate::Id;

/// `integer` 欄位的絕對值上限（JavaScript `Number.MAX_SAFE_INTEGER`，2^53 − 1）。
///
/// 超過此範圍的整數在前端會失去精度。
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// `date` 的格式 `YYYY-MM-DD`。
const DATE_FORMAT: &[BorrowedFormatItem<'static>] = format_description!("[year]-[month]-[day]");

/// 解析後的欄位值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// `text`、`long_text`、`select`（選項值）。
    Text(String),
    /// `integer`。
    Integer(i64),
    /// `decimal`、`money`。保留原本的小數位數（`"12.3400"` 不會變成 `"12.34"`）。
    Decimal(Decimal),
    /// `date`。
    Date(Date),
    /// `datetime`。保留原本的時區位移。
    DateTime(OffsetDateTime),
    /// `boolean`。
    Boolean(bool),
    /// `multi_select`（選項值）。
    MultiSelect(Vec<String>),
    /// `relation`（目標資料的 id）。
    Relation(Id),
    /// `file`（附件的 id）。
    File(Id),
}

/// 欄位值不符合存值格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidValue {
    /// JSON 型別不對，例如 `decimal` 傳入 JSON number、`boolean` 傳入字串。
    TypeMismatch(DataType),
    /// `integer` 不是整數，或超出 ±[`MAX_SAFE_INTEGER`]。
    InvalidInteger,
    /// `decimal` / `money` 的字串無法解析成數字。
    InvalidDecimal,
    /// `date` 不是 `YYYY-MM-DD`，或不是存在的日期。
    InvalidDate,
    /// `datetime` 不是 RFC 3339。
    InvalidDateTime,
    /// `multi_select` 有重複的選項值。
    DuplicateOption,
    /// `relation` 的字串不是 UUID。
    InvalidId,
    /// `file` 的字串不是 UUID。
    InvalidAttachmentId,
    /// 此型態的值尚未支援（`formula`）。
    Unsupported(DataType),
}

impl InvalidValue {
    /// 顯示給使用者的訊息。
    pub fn message(self) -> &'static str {
        match self {
            Self::TypeMismatch(t) => match t {
                DataType::Text | DataType::LongText | DataType::Select => "必須是文字",
                DataType::Integer => "必須是整數",
                DataType::Decimal | DataType::Money => "數字必須以文字傳送，例如 \"12.34\"",
                DataType::Date => "日期必須以文字傳送，格式為 YYYY-MM-DD",
                DataType::DateTime => "日期時間必須以文字傳送，格式為 RFC 3339",
                DataType::Boolean => "必須是 true 或 false",
                DataType::MultiSelect => "必須是文字陣列",
                DataType::Relation => "必須是資料 id（文字）",
                DataType::File => "必須是附件 id（文字）",
                DataType::Formula => "格式不正確",
            },
            Self::InvalidInteger => "必須是整數，且不可超過 ±9007199254740991",
            Self::InvalidDecimal => "數字格式不正確",
            Self::InvalidDate => "日期格式不正確，必須是 YYYY-MM-DD",
            Self::InvalidDateTime => "日期時間格式不正確，必須是 RFC 3339",
            Self::DuplicateOption => "選項不可重複",
            Self::InvalidId => "資料 id 格式不正確",
            Self::InvalidAttachmentId => "附件 id 格式不正確",
            Self::Unsupported(_) => "目前不支援此欄位型態",
        }
    }
}

impl std::fmt::Display for InvalidValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for InvalidValue {}

/// 依欄位型態解析 JSON 存值。`null` 回傳 `Ok(None)`。
pub fn parse_value(data_type: DataType, json: &Value) -> Result<Option<FieldValue>, InvalidValue> {
    if !data_type.is_value_supported() {
        return Err(InvalidValue::Unsupported(data_type));
    }
    if json.is_null() {
        return Ok(None);
    }
    let mismatch = InvalidValue::TypeMismatch(data_type);
    let value = match data_type {
        DataType::Text | DataType::LongText | DataType::Select => {
            FieldValue::Text(json.as_str().ok_or(mismatch)?.to_owned())
        }
        DataType::Integer => {
            if !json.is_number() {
                return Err(mismatch);
            }
            let n = json.as_i64().ok_or(InvalidValue::InvalidInteger)?;
            if n.unsigned_abs() > MAX_SAFE_INTEGER.unsigned_abs() {
                return Err(InvalidValue::InvalidInteger);
            }
            FieldValue::Integer(n)
        }
        DataType::Decimal | DataType::Money => {
            FieldValue::Decimal(parse_decimal(json.as_str().ok_or(mismatch)?)?)
        }
        DataType::Date => FieldValue::Date(
            Date::parse(json.as_str().ok_or(mismatch)?, DATE_FORMAT)
                .map_err(|_| InvalidValue::InvalidDate)?,
        ),
        DataType::DateTime => FieldValue::DateTime(
            OffsetDateTime::parse(json.as_str().ok_or(mismatch)?, &Rfc3339)
                .map_err(|_| InvalidValue::InvalidDateTime)?,
        ),
        DataType::Boolean => FieldValue::Boolean(json.as_bool().ok_or(mismatch)?),
        DataType::MultiSelect => {
            let items = json.as_array().ok_or(mismatch)?;
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                let s = item.as_str().ok_or(mismatch)?;
                if values.iter().any(|v: &String| v == s) {
                    return Err(InvalidValue::DuplicateOption);
                }
                values.push(s.to_owned());
            }
            FieldValue::MultiSelect(values)
        }
        DataType::Relation => FieldValue::Relation(
            Id::try_parse(json.as_str().ok_or(mismatch)?).map_err(|_| InvalidValue::InvalidId)?,
        ),
        DataType::File => FieldValue::File(
            Id::try_parse(json.as_str().ok_or(mismatch)?)
                .map_err(|_| InvalidValue::InvalidAttachmentId)?,
        ),
        DataType::Formula => {
            return Err(InvalidValue::Unsupported(data_type));
        }
    };
    Ok(Some(value))
}

/// 轉回 JSON 存值格式。與 [`parse_value`] 互為反向：`parse_value(t, &to_json(&v)) == Ok(Some(v))`。
pub fn to_json(value: &FieldValue) -> Value {
    match value {
        FieldValue::Text(s) => Value::String(s.clone()),
        FieldValue::Integer(n) => Value::from(*n),
        FieldValue::Decimal(d) => Value::String(d.to_string()),
        FieldValue::Date(d) => Value::String(format_date(*d)),
        FieldValue::DateTime(dt) => Value::String(format_datetime(*dt)),
        FieldValue::Boolean(b) => Value::Bool(*b),
        FieldValue::MultiSelect(items) => {
            Value::Array(items.iter().cloned().map(Value::String).collect())
        }
        FieldValue::Relation(id) | FieldValue::File(id) => {
            Value::String(id.hyphenated().to_string())
        }
    }
}

/// 解析 `decimal` / `money` 的字串，例如 `"12.3400"`、`"-0.5"`。
///
/// 不接受前後空白、科學記號（`"1e3"`）與千分位逗號。
fn parse_decimal(s: &str) -> Result<Decimal, InvalidValue> {
    let well_formed = {
        let digits = s.strip_prefix('-').unwrap_or(s);
        let (int, frac) = match digits.split_once('.') {
            Some((int, frac)) => (int, Some(frac)),
            None => (digits, None),
        };
        !int.is_empty()
            && int.bytes().all(|b| b.is_ascii_digit())
            && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
    };
    if !well_formed {
        return Err(InvalidValue::InvalidDecimal);
    }
    Decimal::from_str(s).map_err(|_| InvalidValue::InvalidDecimal)
}

fn format_date(date: Date) -> String {
    // `[year]` 只有超過 4 位數或負數時才會不同於 `YYYY`，ERP 的日期不會發生
    date.format(DATE_FORMAT)
        .unwrap_or_else(|_| date.to_string())
}

fn format_datetime(dt: OffsetDateTime) -> String {
    // 只有年份超出 0000–9999 時 RFC 3339 才無法表示
    dt.format(&Rfc3339).unwrap_or_else(|_| dt.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use time::macros::{date, datetime};

    fn parse(t: DataType, v: Value) -> Result<Option<FieldValue>, InvalidValue> {
        parse_value(t, &v)
    }

    /// 解析後再轉回 JSON，必須與原本完全相同。
    fn assert_round_trip(t: DataType, v: Value) {
        let parsed = parse(t, v.clone())
            .unwrap_or_else(|e| panic!("{v} 應該合法：{e}"))
            .unwrap();
        assert_eq!(to_json(&parsed), v);
    }

    #[test]
    fn null_is_no_value() {
        for t in DataType::ALL {
            if t.is_value_supported() {
                assert_eq!(parse(*t, Value::Null), Ok(None), "{t:?}");
            }
        }
    }

    #[test]
    fn decimal_round_trips_without_losing_precision() {
        let parsed = parse(DataType::Decimal, json!("12.3400")).unwrap().unwrap();
        assert_eq!(parsed, FieldValue::Decimal(Decimal::new(123400, 4)));
        assert_eq!(to_json(&parsed), json!("12.3400"));

        for t in [DataType::Decimal, DataType::Money] {
            for s in [
                "12.3400",
                "0",
                "0.0000",
                "-0.0001",
                "0.1",
                "99999999999999.9999",
                "1234567.123456",
                "79228162514264337593543950335",
            ] {
                assert_round_trip(t, json!(s));
            }
        }
    }

    #[test]
    fn decimal_rejects_json_number() {
        for t in [DataType::Decimal, DataType::Money] {
            assert_eq!(parse(t, json!(12.34)), Err(InvalidValue::TypeMismatch(t)));
            assert_eq!(parse(t, json!(12)), Err(InvalidValue::TypeMismatch(t)));
            assert_eq!(parse(t, json!(true)), Err(InvalidValue::TypeMismatch(t)));
        }
    }

    #[test]
    fn decimal_rejects_malformed_strings() {
        for s in [
            "", "abc", " 1", "1 ", "1e3", "1E3", "1,000", "+1", ".5", "1.", "--1", "1.2.3", "0x10",
            "NaN", "Infinity",
        ] {
            assert_eq!(
                parse(DataType::Decimal, json!(s)),
                Err(InvalidValue::InvalidDecimal),
                "{s:?}"
            );
        }
        // 超過 Decimal 可表示的範圍
        assert_eq!(
            parse(DataType::Decimal, json!("79228162514264337593543950336")),
            Err(InvalidValue::InvalidDecimal)
        );
    }

    #[test]
    fn date_round_trips() {
        let parsed = parse(DataType::Date, json!("2026-11-13")).unwrap().unwrap();
        assert_eq!(parsed, FieldValue::Date(date!(2026 - 11 - 13)));
        assert_round_trip(DataType::Date, json!("2026-11-13"));
        assert_round_trip(DataType::Date, json!("2024-02-29"));
        assert_round_trip(DataType::Date, json!("0001-01-01"));
    }

    #[test]
    fn date_rejects_invalid_format() {
        for s in [
            "",
            "2026/11/13",
            "2026-1-5",
            "26-11-13",
            "20261113",
            "2026-13-01",
            "2026-02-30",
            "2025-02-29",
            "2026-11-13T00:00:00+08:00",
            " 2026-11-13",
            "2026-11-13 ",
        ] {
            assert_eq!(
                parse(DataType::Date, json!(s)),
                Err(InvalidValue::InvalidDate),
                "{s:?}"
            );
        }
        assert_eq!(
            parse(DataType::Date, json!(20261113)),
            Err(InvalidValue::TypeMismatch(DataType::Date))
        );
    }

    #[test]
    fn datetime_round_trips_keeping_offset() {
        let parsed = parse(DataType::DateTime, json!("2026-11-13T09:30:00+08:00"))
            .unwrap()
            .unwrap();
        assert_eq!(parsed, FieldValue::DateTime(datetime!(2026-11-13 09:30 +8)));
        for s in [
            "2026-11-13T09:30:00+08:00",
            "2026-11-13T01:30:00Z",
            "2026-11-13T01:30:00.5Z",
            "2026-11-13T01:30:00.123456-05:00",
        ] {
            assert_round_trip(DataType::DateTime, json!(s));
        }
    }

    #[test]
    fn datetime_rejects_invalid_format() {
        for s in [
            "",
            "2026-11-13",
            "2026-11-13T09:30:00", // 缺少時區
            "2026-11-13T25:00:00Z",
            "2026-11-13T09:30+08:00",
            "2026/11/13T09:30:00Z",
            "yesterday",
        ] {
            assert_eq!(
                parse(DataType::DateTime, json!(s)),
                Err(InvalidValue::InvalidDateTime),
                "{s:?}"
            );
        }
        assert_eq!(
            parse(DataType::DateTime, json!(1_763_000_000)),
            Err(InvalidValue::TypeMismatch(DataType::DateTime))
        );
    }

    #[test]
    fn integer_must_be_safe_whole_number() {
        assert_round_trip(DataType::Integer, json!(0));
        assert_round_trip(DataType::Integer, json!(-42));
        assert_round_trip(DataType::Integer, json!(MAX_SAFE_INTEGER));
        assert_round_trip(DataType::Integer, json!(-MAX_SAFE_INTEGER));
        for v in [
            json!(1.5),
            json!(1.0),
            json!(MAX_SAFE_INTEGER + 1),
            json!(-MAX_SAFE_INTEGER - 1),
            json!(u64::MAX),
        ] {
            assert_eq!(
                parse(DataType::Integer, v.clone()),
                Err(InvalidValue::InvalidInteger),
                "{v}"
            );
        }
        assert_eq!(
            parse(DataType::Integer, json!("1")),
            Err(InvalidValue::TypeMismatch(DataType::Integer))
        );
    }

    #[test]
    fn text_boolean_and_select_values() {
        for t in [DataType::Text, DataType::LongText, DataType::Select] {
            assert_round_trip(t, json!("維修單 A"));
            assert_round_trip(t, json!(""));
            assert_eq!(parse(t, json!(1)), Err(InvalidValue::TypeMismatch(t)));
        }
        assert_round_trip(DataType::LongText, json!("第一行\n第二行"));
        assert_round_trip(DataType::Boolean, json!(true));
        assert_round_trip(DataType::Boolean, json!(false));
        assert_eq!(
            parse(DataType::Boolean, json!("true")),
            Err(InvalidValue::TypeMismatch(DataType::Boolean))
        );
        assert_eq!(
            parse(DataType::Boolean, json!(1)),
            Err(InvalidValue::TypeMismatch(DataType::Boolean))
        );
    }

    #[test]
    fn multi_select_values() {
        assert_round_trip(DataType::MultiSelect, json!(["a", "b"]));
        assert_round_trip(DataType::MultiSelect, json!([]));
        assert_eq!(
            parse(DataType::MultiSelect, json!(["a", "a"])),
            Err(InvalidValue::DuplicateOption)
        );
        for v in [json!("a"), json!(["a", 1]), json!([null])] {
            assert_eq!(
                parse(DataType::MultiSelect, v.clone()),
                Err(InvalidValue::TypeMismatch(DataType::MultiSelect)),
                "{v}"
            );
        }
    }

    #[test]
    fn relation_values() {
        let id = "0190c3a2-0000-7000-8000-000000000000";
        assert_round_trip(DataType::Relation, json!(id));
        // 大寫或不含連字號的 UUID 可以解析，轉回 JSON 時統一為小寫連字號格式
        let parsed = parse(
            DataType::Relation,
            json!("0190C3A2000070008000000000000000"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(to_json(&parsed), json!(id));
        assert_eq!(parse(DataType::Relation, Value::Null), Ok(None));
        for v in [json!(""), json!("abc"), json!("0190c3a2-0000-7000-8000")] {
            assert_eq!(
                parse(DataType::Relation, v.clone()),
                Err(InvalidValue::InvalidId),
                "{v}"
            );
        }
        for v in [json!(1), json!([id]), json!({ "id": id })] {
            assert_eq!(
                parse(DataType::Relation, v.clone()),
                Err(InvalidValue::TypeMismatch(DataType::Relation)),
                "{v}"
            );
        }
    }

    #[test]
    fn file_values() {
        let id = "0190c3a2-0000-7000-8000-000000000000";
        assert_round_trip(DataType::File, json!(id));
        let parsed = parse(DataType::File, json!("0190C3A2000070008000000000000000"))
            .unwrap()
            .unwrap();
        assert_eq!(parsed, FieldValue::File(Id::try_parse(id).unwrap()));
        assert_eq!(to_json(&parsed), json!(id));
        assert_eq!(parse(DataType::File, Value::Null), Ok(None));
        for v in [json!(""), json!("report.pdf")] {
            assert_eq!(
                parse(DataType::File, v.clone()),
                Err(InvalidValue::InvalidAttachmentId),
                "{v}"
            );
        }
        for v in [json!(1), json!([id]), json!({ "id": id })] {
            assert_eq!(
                parse(DataType::File, v.clone()),
                Err(InvalidValue::TypeMismatch(DataType::File)),
                "{v}"
            );
        }
    }

    #[test]
    fn formula_is_unsupported() {
        let t = DataType::Formula;
        assert_eq!(parse(t, json!("1")), Err(InvalidValue::Unsupported(t)));
        assert_eq!(parse(t, Value::Null), Err(InvalidValue::Unsupported(t)));
    }
}
