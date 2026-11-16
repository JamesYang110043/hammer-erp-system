//! 語法錯誤。

use std::fmt;

use hammer_shared::schema::key::InvalidKey;

use crate::ast::{Arity, Span};

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
