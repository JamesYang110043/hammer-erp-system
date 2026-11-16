//! 計算用的值型別。

use std::fmt;

use hammer_shared::decimal::Decimal;
use hammer_shared::schema::{FieldValue, to_json};
use hammer_shared::time::{Date, OffsetDateTime};

/// 公式計算過程中的值。
///
/// 數字一律是 [`Decimal`]（`integer`、`decimal`、`money` 欄位都轉成數字），不使用浮點數（D9）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// 空值：欄位沒有值、關聯欄位沒有指向資料、`AVG` / `MIN` / `MAX` 沒有可計算的值等。
    Null,
    Number(Decimal),
    Text(String),
    Bool(bool),
    Date(Date),
    DateTime(OffsetDateTime),
}

/// 值的種類，用於錯誤訊息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Null,
    Number,
    Text,
    Bool,
    Date,
    DateTime,
}

impl ValueKind {
    /// 顯示給使用者的名稱。
    pub fn name(self) -> &'static str {
        match self {
            Self::Null => "空值",
            Self::Number => "數字",
            Self::Text => "文字",
            Self::Bool => "布林",
            Self::Date => "日期",
            Self::DateTime => "日期時間",
        }
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Value {
    pub fn kind(&self) -> ValueKind {
        match self {
            Self::Null => ValueKind::Null,
            Self::Number(_) => ValueKind::Number,
            Self::Text(_) => ValueKind::Text,
            Self::Bool(_) => ValueKind::Bool,
            Self::Date(_) => ValueKind::Date,
            Self::DateTime(_) => ValueKind::DateTime,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// 轉成文字（`&`、`CONCAT` 使用）：
    ///
    /// | 值 | 文字 |
    /// |---|---|
    /// | 空值 | 空字串 |
    /// | 數字 | 保留小數位數，例如 `12.3400` |
    /// | 布林 | `TRUE` / `FALSE` |
    /// | 日期 | `YYYY-MM-DD` |
    /// | 日期時間 | RFC 3339 |
    pub fn to_text(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Number(n) => n.to_string(),
            Self::Text(s) => s.clone(),
            Self::Bool(true) => "TRUE".to_owned(),
            Self::Bool(false) => "FALSE".to_owned(),
            Self::Date(d) => json_text(&FieldValue::Date(*d)),
            Self::DateTime(dt) => json_text(&FieldValue::DateTime(*dt)),
        }
    }
}

/// 使用 §7.3 的 JSON 存值格式輸出日期、日期時間的文字。
fn json_text(value: &FieldValue) -> String {
    match to_json(value) {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    }
}
