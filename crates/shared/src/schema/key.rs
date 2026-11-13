//! 資料類別 `key` 與欄位 `field_key` 的命名規則（`docs/architecture.md` §7.2、§8.5）。
//!
//! 兩者建立後都不可修改，會出現在 JSONB 的鍵、公式的引用名稱、運算式索引中，因此規則較嚴格：
//!
//! - 只能使用小寫英文字母、數字與底線（`a-z`、`0-9`、`_`）
//! - 開頭必須是英文字母
//! - 長度 1 到 [`MAX_KEY_LEN`] 個字元

/// 代碼長度上限。
///
/// PostgreSQL 識別字上限是 63 bytes；動態索引名稱會包含資料類別與欄位代碼（§7.5），
/// 因此保留空間給前綴與另一個代碼。
pub const MAX_KEY_LEN: usize = 30;

/// 代碼不符合命名規則。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidKey {
    /// 空字串。
    Empty,
    /// 超過 [`MAX_KEY_LEN`] 個字元。
    TooLong,
    /// 開頭不是小寫英文字母。
    InvalidStart,
    /// 含有小寫英文字母、數字、底線以外的字元。
    InvalidChar,
}

impl InvalidKey {
    /// 顯示給使用者的訊息。
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "代碼不可為空白",
            Self::TooLong => "代碼不可超過 30 個字元",
            Self::InvalidStart => "代碼必須以小寫英文字母開頭",
            Self::InvalidChar => "代碼只能使用小寫英文字母、數字與底線",
        }
    }
}

impl std::fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for InvalidKey {}

/// 檢查資料類別 `key` 或欄位 `field_key` 是否符合命名規則。不會去除前後空白。
pub fn validate_key(key: &str) -> Result<(), InvalidKey> {
    let mut chars = key.chars();
    let first = chars.next().ok_or(InvalidKey::Empty)?;
    // 只允許 ASCII，因此字元數等於 byte 數
    if !first.is_ascii_lowercase() {
        return Err(InvalidKey::InvalidStart);
    }
    if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
        return Err(InvalidKey::InvalidChar);
    }
    if key.len() > MAX_KEY_LEN {
        return Err(InvalidKey::TooLong);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_keys() {
        for key in [
            "a",
            "customer",
            "repair_order",
            "field_2",
            "a_",
            "x1_y2_z3",
            "abcdefghijklmnopqrstuvwxyz0123", // 剛好 30 個字元
        ] {
            assert_eq!(validate_key(key), Ok(()), "{key} 應該合法");
        }
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(validate_key(""), Err(InvalidKey::Empty));
    }

    #[test]
    fn rejects_invalid_start() {
        for key in ["1abc", "_abc", "Abc", " abc", "客戶", "-a"] {
            assert_eq!(validate_key(key), Err(InvalidKey::InvalidStart), "{key}");
        }
    }

    #[test]
    fn rejects_invalid_chars() {
        for key in [
            "repairOrder",
            "repair-order",
            "repair order",
            "abc ",
            "a.b",
            "a客戶",
            "café",
            "a\n",
        ] {
            assert_eq!(validate_key(key), Err(InvalidKey::InvalidChar), "{key}");
        }
    }

    #[test]
    fn rejects_too_long() {
        let key = "a".repeat(MAX_KEY_LEN + 1);
        assert_eq!(validate_key(&key), Err(InvalidKey::TooLong));
    }

    #[test]
    fn messages_are_not_empty() {
        for e in [
            InvalidKey::Empty,
            InvalidKey::TooLong,
            InvalidKey::InvalidStart,
            InvalidKey::InvalidChar,
        ] {
            assert!(!e.message().is_empty());
        }
        assert!(
            InvalidKey::TooLong
                .message()
                .contains(&MAX_KEY_LEN.to_string())
        );
    }
}
