//! 語法錯誤與計算錯誤。

use std::fmt;

use hammer_shared::schema::key::InvalidKey;
use hammer_shared::schema::{DataType, InvalidValue};

use crate::ast::{Arity, Span};
use crate::value::ValueKind;

/// 解析失敗。`span` 是錯誤在公式文字中的位置（字元位移）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub kind: ParseErrorKind,
    pub span: Span,
}

impl ParseError {
    pub(crate) fn new(kind: ParseErrorKind, span: Span) -> Self {
        Self { kind, span }
    }

    /// 顯示給使用者的訊息（繁體中文，不含位置）。
    pub fn message(&self) -> String {
        self.kind.to_string()
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "第 {} 個字元：{}", self.span.start + 1, self.kind)
    }
}

impl std::error::Error for ParseError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseErrorKind {
    /// 公式是空白。
    Empty,
    /// 超過長度上限（字元數）。
    TooLong { max: usize },
    /// 超過巢狀深度上限。
    TooDeep { max: usize },
    /// 無法辨識的字元。
    UnexpectedChar(char),
    /// 字串缺少結尾的 `"`。
    UnclosedString,
    /// 字串中不支援的跳脫字元（`\` 後面的字元；`None` 表示 `\` 在公式結尾）。
    InvalidEscape(Option<char>),
    /// 數字格式錯誤或超出範圍。
    InvalidNumber,
    /// 欄位引用缺少結尾的 `}`。
    UnclosedFieldRef,
    /// 欄位、關聯欄位或明細子表代碼不符合命名規則。
    InvalidKey { key: String, reason: InvalidKey },
    /// 關聯引用超過一層，例如 `{a.b.c}`。
    NestedRelation,
    /// 括號沒有閉合。
    UnclosedParen,
    /// 公式在預期之外結束。
    UnexpectedEnd,
    /// 不該出現在這裡的符號；`found` 是該符號的文字。
    UnexpectedToken { found: String },
    /// 名稱後面沒有括號，不是函式呼叫，也不是關鍵字。
    BareIdentifier(String),
    /// 未知的函式。
    UnknownFunction(String),
    /// 函式參數個數錯誤。
    WrongArgCount {
        func: &'static str,
        expected: Arity,
        found: usize,
    },
    /// 彙總函式的參數不是「明細子表代碼.欄位代碼」。
    InvalidAggregateArg { func: &'static str },
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("公式不可為空白"),
            Self::TooLong { max } => write!(f, "公式不可超過 {max} 個字元"),
            Self::TooDeep { max } => write!(f, "公式的巢狀層數不可超過 {max} 層"),
            Self::UnexpectedChar(c) => write!(f, "無法辨識的字元「{c}」"),
            Self::UnclosedString => f.write_str("字串缺少結尾的雙引號"),
            Self::InvalidEscape(Some(c)) => {
                write!(f, "不支援的跳脫字元「\\{c}」，只能使用 \\\" 或 \\\\")
            }
            Self::InvalidEscape(None) => f.write_str("字串結尾的反斜線缺少跳脫字元"),
            Self::InvalidNumber => f.write_str("數字格式錯誤或超出範圍"),
            Self::UnclosedFieldRef => f.write_str("欄位引用缺少結尾的右大括號"),
            Self::InvalidKey { key, reason } => write!(f, "代碼「{key}」不合法：{reason}"),
            Self::NestedRelation => f.write_str("關聯引用最多只能一層，例如 {客戶.折扣率}"),
            Self::UnclosedParen => f.write_str("括號沒有閉合"),
            Self::UnexpectedEnd => f.write_str("公式不完整"),
            Self::UnexpectedToken { found } => write!(f, "這裡不能出現「{found}」"),
            Self::BareIdentifier(name) => {
                write!(f, "無法辨識「{name}」；欄位引用請寫成 {{{name}}}")
            }
            Self::UnknownFunction(name) => write!(f, "未知的函式「{name}」"),
            Self::WrongArgCount {
                func,
                expected,
                found,
            } => write!(f, "函式 {func} 需要 {expected}參數，但提供了 {found} 個"),
            Self::InvalidAggregateArg { func } => {
                write!(
                    f,
                    "函式 {func} 的參數必須是「明細子表代碼.欄位代碼」，例如 {func}(lines.amount)"
                )
            }
        }
    }
}

/// 計算失敗。公式的結果視為空值，錯誤原因（繁體中文）顯示在欄位上，不中斷存檔（§7.6）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// 除數為零。
    DivisionByZero,
    /// 計算結果超出 `Decimal` 可表示的範圍，或超出結果型態的範圍。
    Overflow,
    /// 運算子或函式參數的型態不對；`context` 例如「運算子「*」」、「函式 ROUND 的第 1 個參數」。
    TypeMismatch {
        context: String,
        expected: &'static str,
        found: ValueKind,
    },
    /// 比較運算子兩邊的型態無法比較，例如數字與文字。
    Incomparable {
        op: &'static str,
        left: ValueKind,
        right: ValueKind,
    },
    /// `ROUND` 的小數位數不是 0 到 [`MAX_ROUND_DIGITS`](crate::MAX_ROUND_DIGITS) 的整數。
    InvalidRoundDigits,
    /// 本筆資料沒有這個欄位。
    UnknownField(String),
    /// 關聯欄位或其目標欄位不存在。
    UnknownRelationField { relation: String, field: String },
    /// 明細子表或其欄位不存在。
    UnknownDetailField { detail: String, field: String },
    /// 公式不支援此型態的欄位（`multi_select`、`relation`、`file`）。
    UnsupportedFieldType { field: String, data_type: DataType },
    /// 資料庫中的欄位值不符合 §7.3 的存值格式。
    InvalidStoredValue { field: String, reason: InvalidValue },
    /// 計算結果無法轉成公式欄位設定的結果型態。
    ResultTypeMismatch {
        expected: DataType,
        found: ValueKind,
    },
    /// 公式欄位設定的結果型態不支援（`select`、`multi_select`、`relation`、`file`、`formula`）。
    UnsupportedResultType(DataType),
    /// `decimal` / `money` 結果沒有設定小數位數，或小數位數超出 0 到 28。
    InvalidDecimalPlaces(Option<i16>),
}

impl EvalError {
    /// 顯示給使用者的訊息（繁體中文）。
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DivisionByZero => f.write_str("除數不可為零"),
            Self::Overflow => f.write_str("計算結果超出可表示的範圍"),
            Self::TypeMismatch {
                context,
                expected,
                found,
            } => write!(f, "{context}需要{expected}，但收到{found}"),
            Self::Incomparable { op, left, right } => {
                write!(f, "運算子「{op}」無法比較{left}與{right}")
            }
            Self::InvalidRoundDigits => write!(
                f,
                "函式 ROUND 的小數位數必須是 0 到 {} 的整數",
                crate::MAX_ROUND_DIGITS
            ),
            Self::UnknownField(key) => write!(f, "找不到欄位「{key}」"),
            Self::UnknownRelationField { relation, field } => {
                write!(f, "找不到關聯欄位「{relation}.{field}」")
            }
            Self::UnknownDetailField { detail, field } => {
                write!(f, "找不到明細欄位「{detail}.{field}」")
            }
            Self::UnsupportedFieldType { field, data_type } => write!(
                f,
                "公式不能使用欄位「{field}」（型態 {}）",
                data_type.as_str()
            ),
            Self::InvalidStoredValue { field, reason } => {
                write!(f, "欄位「{field}」的值不正確：{reason}")
            }
            Self::ResultTypeMismatch { expected, found } => {
                write!(f, "計算結果是{found}，無法存成 {} 型態", expected.as_str())
            }
            Self::UnsupportedResultType(t) => {
                write!(f, "公式的結果型態不能是 {}", t.as_str())
            }
            Self::InvalidDecimalPlaces(None) => f.write_str("公式的結果沒有設定小數位數"),
            Self::InvalidDecimalPlaces(Some(p)) => {
                write!(f, "公式結果的小數位數 {p} 不正確，必須介於 0 到 28")
            }
        }
    }
}

impl std::error::Error for EvalError {}
