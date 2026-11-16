//! 公式的即時語法檢查（`docs/architecture.md` §3.3：桌面端直接使用 `crates/formula`）。
//!
//! 只檢查語法；引用的欄位是否存在、型態、循環引用需要其他欄位定義，由伺服器在儲存時檢查（§7.6）。

use serde::Serialize;
use specta::Type;

/// 公式的語法錯誤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FormulaSyntaxError {
    /// 錯誤訊息（繁體中文，不含位置）。
    pub message: String,
    /// 錯誤在公式中的位置：**字元**位移（Unicode 字元，不是 UTF-16），從 0 開始。
    pub start: u32,
    /// 錯誤範圍的結尾（不含）；與 `start` 相同時代表該位置缺少內容（例如公式不完整）。
    pub end: u32,
}

/// 檢查公式語法，正確時回傳 `None`。
pub fn check_syntax(formula: &str) -> Option<FormulaSyntaxError> {
    let err = hammer_formula::parse(formula).err()?;
    // 公式長度有上限（`MAX_FORMULA_LEN`），位移不會超過 u32
    let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    Some(FormulaSyntaxError {
        message: err.message(),
        start: to_u32(err.span.start),
        end: to_u32(err.span.end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_formula_has_no_error() {
        assert_eq!(check_syntax("{qty} * {unit_price}"), None);
        assert_eq!(check_syntax("SUM(lines.amount) + {customer.rate}"), None);
    }

    #[test]
    fn reports_message_and_char_span() {
        let err = check_syntax("{qty} * ").unwrap();
        assert_eq!(err.message, "公式不完整");
        assert_eq!((err.start, err.end), (8, 8));

        // 位置以字元計算（中文字串不以 byte 計）
        let err = check_syntax("\"數量\" & @").unwrap();
        assert_eq!(err.message, "無法辨識的字元「@」");
        assert_eq!((err.start, err.end), (7, 8));
    }

    #[test]
    fn empty_formula_is_an_error() {
        let err = check_syntax("   ").unwrap();
        assert_eq!(err.message, "公式不可為空白");
    }
}
