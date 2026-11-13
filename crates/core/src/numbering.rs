//! 單號規則（docs/architecture.md §8.4 `number_sequences`、§8.5 `entity_definitions.numbering_pattern`）。
//!
//! 規則由文字與代碼組成，代碼寫在 `{}` 內：
//!
//! | 代碼 | 說明 | 例子（2026-11-05） |
//! |---|---|---|
//! | `yyyy` | 西元年，4 位 | `2026` |
//! | `yy` | 西元年末 2 位 | `26` |
//! | `MM` | 月，2 位 | `11` |
//! | `dd` | 日，2 位 | `05` |
//! | `000…` | 流水號，3 到 8 個 `0`，位數不足時補零 | `{0000}` → `0001` |
//!
//! - 日期代碼可以寫在同一個 `{}` 內，例如 `{yyyyMM}`；流水號必須單獨寫成 `{000…}`
//! - 規則必須**恰好一個**流水號；`{}` 以外的文字原樣保留，但不可有單獨的 `{`、`}` 或控制字元
//! - 期間（`period_key`）為規則中所有日期代碼依序產生的文字，例如 `SO{yyyyMM}-{0000}` 在
//!   2026-11 為 `202611`；同一期間共用一組流水號，期間改變時重新從 1 開始。沒有日期代碼時為空字串，
//!   流水號永不重置
//! - 日期以建立資料當下的 Asia/Taipei 日期為準（[`numbering_date`]）
//! - 流水號超過位數時繼續遞增，不截斷：`{000}` 的第 1000 號為 `1000`
//!
//! 解析：[`NumberingPattern::parse`]；寫入資料類別前的檢查與整理：[`normalize_pattern`]。

use hammer_shared::time::TAIPEI_OFFSET;
use time::{Date, OffsetDateTime};

/// 單號規則的長度上限（字元數）。
pub const MAX_PATTERN_LEN: usize = 50;
/// 流水號最少位數。
pub const MIN_SEQUENCE_DIGITS: usize = 3;
/// 流水號最多位數。
pub const MAX_SEQUENCE_DIGITS: usize = 8;

/// 日期代碼。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateCode {
    /// `yyyy`
    Year4,
    /// `yy`
    Year2,
    /// `MM`
    Month,
    /// `dd`
    Day,
}

impl DateCode {
    /// 代碼文字與對應的值，較長的放前面（`yyyy` 優先於 `yy`）。
    const TOKENS: &[(&str, DateCode)] = &[
        ("yyyy", Self::Year4),
        ("yy", Self::Year2),
        ("MM", Self::Month),
        ("dd", Self::Day),
    ];

    fn render(self, date: Date) -> String {
        match self {
            Self::Year4 => format!("{:04}", date.year()),
            Self::Year2 => format!("{:02}", date.year().rem_euclid(100)),
            Self::Month => format!("{:02}", u8::from(date.month())),
            Self::Day => format!("{:02}", date.day()),
        }
    }
}

/// 規則的組成單位。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Literal(String),
    Date(DateCode),
    /// 流水號，內容為最少位數。
    Sequence(usize),
}

/// 解析後的單號規則。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberingPattern {
    parts: Vec<Part>,
}

/// 單號規則不合法。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternError {
    /// 超過 [`MAX_PATTERN_LEN`] 個字元。
    TooLong,
    /// 有 `{` 沒有對應的 `}`。
    UnclosedBrace,
    /// 有 `}` 沒有對應的 `{`。
    UnmatchedCloseBrace,
    /// 含有控制字元（例如換行）。
    ControlChar,
    /// 不認得的代碼（`{}` 內的文字）。
    UnknownCode(String),
    /// 流水號位數不在 [`MIN_SEQUENCE_DIGITS`] 到 [`MAX_SEQUENCE_DIGITS`] 之間。
    SequenceDigits,
    /// 沒有流水號。
    MissingSequence,
    /// 有多個流水號。
    MultipleSequences,
}

impl PatternError {
    /// 顯示給使用者的訊息。
    pub fn message(&self) -> String {
        match self {
            Self::TooLong => format!("單號規則不可超過 {MAX_PATTERN_LEN} 個字元"),
            Self::UnclosedBrace => "單號規則的「{」沒有對應的「}」".to_owned(),
            Self::UnmatchedCloseBrace => "單號規則的「}」沒有對應的「{」".to_owned(),
            Self::ControlChar => "單號規則不可包含換行等控制字元".to_owned(),
            Self::UnknownCode(code) => format!(
                "不認得的代碼「{{{code}}}」；可用的代碼為 {{yyyy}}、{{yy}}、{{MM}}、{{dd}} 與流水號 {{000}}"
            ),
            Self::SequenceDigits => format!(
                "流水號必須是 {MIN_SEQUENCE_DIGITS} 到 {MAX_SEQUENCE_DIGITS} 個 0，例如 {{0000}}"
            ),
            Self::MissingSequence => "單號規則必須有一個流水號，例如 {0000}".to_owned(),
            Self::MultipleSequences => "單號規則只能有一個流水號".to_owned(),
        }
    }
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for PatternError {}

impl NumberingPattern {
    /// 解析單號規則。不會去除前後空白（見 [`normalize_pattern`]）。
    pub fn parse(pattern: &str) -> Result<Self, PatternError> {
        if pattern.chars().count() > MAX_PATTERN_LEN {
            return Err(PatternError::TooLong);
        }
        if pattern.chars().any(char::is_control) {
            return Err(PatternError::ControlChar);
        }

        let mut parts = Vec::new();
        let mut literal = String::new();
        let mut rest = pattern;
        while let Some(c) = rest.chars().next() {
            match c {
                '{' => {
                    let end = rest.find('}').ok_or(PatternError::UnclosedBrace)?;
                    let code = &rest[1..end];
                    if code.contains('{') {
                        return Err(PatternError::UnclosedBrace);
                    }
                    if !literal.is_empty() {
                        parts.push(Part::Literal(std::mem::take(&mut literal)));
                    }
                    parse_code(code, &mut parts)?;
                    rest = &rest[end + 1..];
                }
                '}' => return Err(PatternError::UnmatchedCloseBrace),
                _ => {
                    literal.push(c);
                    rest = &rest[c.len_utf8()..];
                }
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Literal(literal));
        }

        match parts
            .iter()
            .filter(|p| matches!(p, Part::Sequence(_)))
            .count()
        {
            0 => Err(PatternError::MissingSequence),
            1 => Ok(Self { parts }),
            _ => Err(PatternError::MultipleSequences),
        }
    }

    /// 期間：規則中所有日期代碼依序產生的文字；沒有日期代碼時為空字串。
    pub fn period_key(&self, date: Date) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                Part::Date(code) => Some(code.render(date)),
                _ => None,
            })
            .collect()
    }

    /// 產生單號。`sequence` 位數不足時補零，超過時不截斷。
    pub fn format(&self, date: Date, sequence: i32) -> String {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Literal(text) => text.clone(),
                Part::Date(code) => code.render(date),
                Part::Sequence(digits) => format!("{sequence:0digits$}"),
            })
            .collect()
    }
}

/// `{}` 內的代碼：全是 `0` 為流水號，否則必須由日期代碼組成。
fn parse_code(code: &str, parts: &mut Vec<Part>) -> Result<(), PatternError> {
    if !code.is_empty() && code.chars().all(|c| c == '0') {
        let digits = code.len();
        if !(MIN_SEQUENCE_DIGITS..=MAX_SEQUENCE_DIGITS).contains(&digits) {
            return Err(PatternError::SequenceDigits);
        }
        parts.push(Part::Sequence(digits));
        return Ok(());
    }

    let unknown = || PatternError::UnknownCode(code.to_owned());
    if code.is_empty() {
        return Err(unknown());
    }
    let mut rest = code;
    while !rest.is_empty() {
        let (token, date_code) = DateCode::TOKENS
            .iter()
            .find(|(token, _)| rest.starts_with(token))
            .ok_or_else(unknown)?;
        parts.push(Part::Date(*date_code));
        rest = &rest[token.len()..];
    }
    Ok(())
}

/// 寫入資料類別前檢查並整理單號規則：去除前後空白，空白視為未設定（`None`）。
pub fn normalize_pattern(pattern: Option<&str>) -> Result<Option<String>, PatternError> {
    let Some(pattern) = pattern.map(str::trim).filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    NumberingPattern::parse(pattern)?;
    Ok(Some(pattern.to_owned()))
}

/// 產生單號使用的日期：`now` 在 Asia/Taipei 的日期（D13）。
pub fn numbering_date(now: OffsetDateTime) -> Date {
    now.to_offset(TAIPEI_OFFSET).date()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::{date, datetime};

    const DATE: Date = date!(2026 - 11 - 05);

    fn parse(pattern: &str) -> NumberingPattern {
        NumberingPattern::parse(pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"))
    }

    fn err(pattern: &str) -> PatternError {
        match NumberingPattern::parse(pattern) {
            Err(e) => e,
            Ok(p) => panic!("{pattern} 應該不合法，實際為 {p:?}"),
        }
    }

    #[test]
    fn renders_each_code() {
        for (pattern, expected) in [
            ("{yyyy}-{000}", "2026-007"),
            ("{yy}-{000}", "26-007"),
            ("{MM}-{000}", "11-007"),
            ("{dd}-{000}", "05-007"),
            ("{yyyyMMdd}{0000}", "202611050007"),
            ("SO{yyyyMM}-{0000}", "SO202611-0007"),
            ("{000}", "007"),
            ("維修-{yy}{MM}/{00000000}", "維修-2611/00000007"),
        ] {
            assert_eq!(parse(pattern).format(DATE, 7), expected, "{pattern}");
        }
    }

    #[test]
    fn two_digit_year_and_padding() {
        let p = parse("{yy}{MM}{dd}{000}");
        assert_eq!(p.format(date!(2009 - 01 - 02), 1), "090102001");
        assert_eq!(p.format(date!(2100 - 12 - 31), 1), "001231001");
    }

    #[test]
    fn period_key_concatenates_date_codes() {
        for (pattern, expected) in [
            ("SO{yyyyMM}-{0000}", "202611"),
            ("SO{yyyy}{MM}-{0000}", "202611"),
            ("{yy}-{MM}-{dd}-{000}", "261105"),
            ("{yyyy}-{000}", "2026"),
            ("{0000}{dd}", "05"),
            ("RO-{0000}", ""),
        ] {
            assert_eq!(parse(pattern).period_key(DATE), expected, "{pattern}");
        }
    }

    #[test]
    fn period_changes_with_month_only_when_month_is_used() {
        let monthly = parse("SO{yyyyMM}-{0000}");
        assert_ne!(
            monthly.period_key(date!(2026 - 11 - 30)),
            monthly.period_key(date!(2026 - 12 - 01))
        );
        let yearly = parse("SO{yyyy}-{0000}");
        assert_eq!(
            yearly.period_key(date!(2026 - 11 - 30)),
            yearly.period_key(date!(2026 - 12 - 01))
        );
    }

    #[test]
    fn sequence_overflow_is_not_truncated() {
        let p = parse("A{000}");
        assert_eq!(p.format(DATE, 999), "A999");
        assert_eq!(p.format(DATE, 1000), "A1000");
        assert_eq!(p.format(DATE, 123_456), "A123456");
    }

    #[test]
    fn rejects_invalid_patterns() {
        for (pattern, expected) in [
            ("SO-", PatternError::MissingSequence),
            ("{yyyyMM}", PatternError::MissingSequence),
            ("", PatternError::MissingSequence),
            ("{000}-{0000}", PatternError::MultipleSequences),
            ("{00}", PatternError::SequenceDigits),
            ("{000000000}", PatternError::SequenceDigits),
            ("{yyyy", PatternError::UnclosedBrace),
            ("{yy{000}", PatternError::UnclosedBrace),
            ("A}{000}", PatternError::UnmatchedCloseBrace),
            ("{000}}", PatternError::UnmatchedCloseBrace),
            ("A\n{000}", PatternError::ControlChar),
        ] {
            assert_eq!(err(pattern), expected, "{pattern:?}");
        }
    }

    #[test]
    fn rejects_unknown_codes() {
        for code in [
            "", "YYYY", "mm", "HH", "y", "yyy", "yyyyM", "0a", "yyyy000", " 000", "no",
        ] {
            assert_eq!(
                err(&format!("{{{code}}}-{{000}}")),
                PatternError::UnknownCode(code.to_owned()),
                "{code:?}"
            );
        }
    }

    #[test]
    fn length_limit_counts_characters() {
        let ok = format!("{}{{000}}", "單".repeat(MAX_PATTERN_LEN - 5));
        assert!(NumberingPattern::parse(&ok).is_ok());
        let long = format!("{}{{000}}", "單".repeat(MAX_PATTERN_LEN - 4));
        assert_eq!(err(&long), PatternError::TooLong);
    }

    #[test]
    fn messages_are_not_empty() {
        for e in [
            PatternError::TooLong,
            PatternError::UnclosedBrace,
            PatternError::UnmatchedCloseBrace,
            PatternError::ControlChar,
            PatternError::UnknownCode("x".into()),
            PatternError::SequenceDigits,
            PatternError::MissingSequence,
            PatternError::MultipleSequences,
        ] {
            assert!(!e.message().is_empty());
            assert_eq!(e.to_string(), e.message());
        }
        assert!(
            PatternError::UnknownCode("HH".into())
                .message()
                .contains("{HH}")
        );
    }

    #[test]
    fn normalize_trims_and_clears_blank() {
        assert_eq!(normalize_pattern(None), Ok(None));
        assert_eq!(normalize_pattern(Some("  ")), Ok(None));
        assert_eq!(
            normalize_pattern(Some(" SO{yyyyMM}-{0000} ")),
            Ok(Some("SO{yyyyMM}-{0000}".to_owned()))
        );
        assert_eq!(
            normalize_pattern(Some("SO-")),
            Err(PatternError::MissingSequence)
        );
    }

    #[test]
    fn numbering_date_uses_taipei() {
        // UTC 11/30 16:30 已是台北 12/01 00:30
        assert_eq!(
            numbering_date(datetime!(2026-11-30 16:30 UTC)),
            date!(2026 - 12 - 01)
        );
        assert_eq!(
            numbering_date(datetime!(2026-11-30 15:59 UTC)),
            date!(2026 - 11 - 30)
        );
    }
}
