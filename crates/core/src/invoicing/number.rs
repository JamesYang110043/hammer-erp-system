//! 發票號碼的正規化與格式（docs/architecture.md §8.11「發票號碼」）。純邏輯。
//!
//! - 存檔時去除前後空白、英文轉大寫（[`normalize`]）
//! - 銷項發票、進項憑證的三聯式與電子發票：2 碼英文 + 8 碼數字（[`is_invoice_no`]）
//! - 海關代徵營業稅繳納證：英數字，最長 20 碼（[`is_customs_no`]）

/// 發票號碼格式錯誤。
pub const MSG_INVOICE_NO_FORMAT: &str = "發票號碼格式應為 2 碼英文加 8 碼數字";
/// 海關繳納證號碼格式錯誤。
pub const MSG_CUSTOMS_NO_FORMAT: &str = "繳納證號碼只能是英文與數字，最長 20 碼";

/// 去除前後空白、英文轉大寫。
pub fn normalize(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

/// 2 碼英文（大寫）+ 8 碼數字。
pub fn is_invoice_no(no: &str) -> bool {
    let b = no.as_bytes();
    b.len() == 10
        && b[..2].iter().all(u8::is_ascii_uppercase)
        && b[2..].iter().all(u8::is_ascii_digit)
}

/// 英文（大寫）與數字，1–20 碼。
pub fn is_customs_no(no: &str) -> bool {
    (1..=20).contains(&no.len())
        && no
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_case_and_spaces() {
        assert_eq!(normalize("  ab12345678 "), "AB12345678");
        assert_eq!(normalize("Ab12345678"), "AB12345678");
    }

    #[test]
    fn invoice_no_format() {
        assert!(is_invoice_no("AB12345678"));
        for bad in [
            "ab12345678",
            "A112345678",
            "AB1234567",
            "AB123456789",
            "AB1234567X",
            "ＡＢ12345678",
            "",
        ] {
            assert!(!is_invoice_no(bad), "{bad}");
        }
    }

    #[test]
    fn customs_no_format() {
        assert!(is_customs_no("AA12345678901234"));
        assert!(is_customs_no("1"));
        assert!(is_customs_no(&"9".repeat(20)));
        for bad in ["", "AB-123", "ab123", &"9".repeat(21), "AB 123"] {
            assert!(!is_customs_no(bad), "{bad}");
        }
    }
}
