//! 發票模組的金額與稅額（docs/architecture.md §8.11「金額與稅額」）。純邏輯，不查詢資料庫。
//!
//! 銷項發票、銷貨折讓的明細金額與合計共用 [`crate::sales::amounts`]（三聯式、折讓為未稅，二聯式為
//! 含稅），再套用**手動修正的稅額**（[`apply_manual_tax`]）：
//!
//! - 必須是 `d` 位小數以內，與系統計算的稅額相差不超過 1 元（[`MAX_TAX_DIFF`]）
//! - 未稅（三聯式、折讓）：`subtotal` 不變，`total = subtotal + 稅額`
//! - 含稅（二聯式）：`total` 不變，`subtotal = total − 稅額`
//! - 稅率為 0 時不可填
//!
//! 進項憑證、進貨折讓沒有明細，稅額由使用者輸入，以 [`check_entered_tax`] 檢查。

use hammer_shared::decimal::Decimal;

use crate::purchasing::amounts::{AMOUNT_SCALE, round_to, with_scale};
use crate::sales::amounts::Totals;

/// 手動稅額（輸入的稅額）與系統計算可相差的上限（元）。
pub const MAX_TAX_DIFF: Decimal = Decimal::ONE;

/// 零稅率、免稅卻填了手動稅額。
pub const MSG_TAX_ON_ZERO_RATE: &str = "零稅率、免稅的發票沒有稅額";

/// 金額的顯示：`d` 位小數，不補千分位（錯誤訊息用）。
pub fn display(value: Decimal, decimal_places: u32) -> String {
    let mut v = round_to(value, decimal_places, decimal_places);
    v.rescale(decimal_places);
    v.to_string()
}

/// 金額的顯示：`d` 位小數、千分位（錯誤訊息用，例如 `1,050`）。
pub fn display_grouped(value: Decimal, decimal_places: u32) -> String {
    let s = display(value.abs(), decimal_places);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
    };
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if let Some(f) = frac {
        grouped.push('.');
        grouped.push_str(f);
    }
    if value.is_sign_negative() && !value.is_zero() {
        grouped.insert(0, '-');
    }
    grouped
}

/// 小數位數超過 `d` 位的訊息。
fn msg_places(decimal_places: u32) -> String {
    if decimal_places == 0 {
        "稅額必須是整數".into()
    } else {
        format!("稅額最多 {decimal_places} 位小數")
    }
}

/// 套用手動修正的稅額。`computed` 為系統計算的合計（[`crate::sales::amounts::totals`]），
/// `tax_included` 為二聯式（單價含稅）。錯誤時回傳給 `tax_amount_manual` 的訊息。
pub fn apply_manual_tax(
    computed: Totals,
    manual: Option<Decimal>,
    tax_rate: Decimal,
    tax_included: bool,
    decimal_places: u32,
) -> Result<Totals, String> {
    let Some(manual) = manual else {
        return Ok(computed);
    };
    if tax_rate.is_zero() {
        return Err(MSG_TAX_ON_ZERO_RATE.into());
    }
    if manual.normalize().scale() > decimal_places {
        return Err(msg_places(decimal_places));
    }
    if (manual - computed.tax_amount).abs() > MAX_TAX_DIFF {
        return Err(format!(
            "稅額與系統計算相差超過 1 元（系統計算 {}）",
            display(computed.tax_amount, decimal_places)
        ));
    }
    let tax_amount = with_scale(manual, AMOUNT_SCALE);
    Ok(if tax_included {
        Totals {
            subtotal: with_scale(computed.total - manual, AMOUNT_SCALE),
            tax_amount,
            total: computed.total,
        }
    } else {
        Totals {
            subtotal: computed.subtotal,
            tax_amount,
            total: with_scale(computed.subtotal + manual, AMOUNT_SCALE),
        }
    })
}

/// 使用者輸入的稅額（進項憑證、進貨折讓）：`d` 位小數以內，與 `round(subtotal × 稅率, d)` 相差不超過
/// 1 元；稅率為 0 時必須是 0。錯誤時回傳給 `tax_amount` 的訊息。
pub fn check_entered_tax(
    subtotal: Decimal,
    tax_amount: Decimal,
    tax_rate: Decimal,
    decimal_places: u32,
) -> Result<(), String> {
    if tax_amount.normalize().scale() > decimal_places {
        return Err(msg_places(decimal_places));
    }
    if tax_rate.is_zero() {
        return if tax_amount.is_zero() {
            Ok(())
        } else {
            Err(MSG_TAX_ON_ZERO_RATE.into())
        };
    }
    let expected = round_to(subtotal * tax_rate, decimal_places, AMOUNT_SCALE);
    if (tax_amount - expected).abs() > MAX_TAX_DIFF {
        return Err(format!(
            "稅額與未稅金額 × 稅率相差超過 1 元（應為 {}）",
            display(expected, decimal_places)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sales::amounts::{line_amount, totals};
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn rate() -> Decimal {
        d("0.05")
    }

    /// §8.11 範例一：三聯式、應稅。
    fn example_one() -> Totals {
        let amounts = [
            line_amount(d("3"), d("100.0000"), Decimal::ZERO, 0),
            line_amount(d("2"), d("333.3333"), Decimal::ZERO, 0),
        ];
        assert_eq!(amounts[1], d("667.0000"));
        totals(amounts, rate(), false, 0)
    }

    /// §8.11 範例二：二聯式、應稅。
    fn example_two() -> Totals {
        let amounts = [
            line_amount(d("4"), d("200.0000"), Decimal::ZERO, 0),
            line_amount(d("1"), d("200.0000"), Decimal::ZERO, 0),
        ];
        totals(amounts, rate(), true, 0)
    }

    #[test]
    fn triplicate_example() {
        let t = example_one();
        assert_eq!(
            (t.subtotal, t.tax_amount, t.total),
            (d("967"), d("48"), d("1015"))
        );
        assert_eq!(apply_manual_tax(t, None, rate(), false, 0), Ok(t));
        let m = apply_manual_tax(t, Some(d("49")), rate(), false, 0).unwrap();
        assert_eq!(
            (m.subtotal, m.tax_amount, m.total),
            (d("967"), d("49"), d("1016"))
        );
        assert_eq!(m.total.to_string(), "1016.0000");
        assert_eq!(
            apply_manual_tax(t, Some(d("50")), rate(), false, 0),
            Err("稅額與系統計算相差超過 1 元（系統計算 48）".into())
        );
        assert!(apply_manual_tax(t, Some(d("47")), rate(), false, 0).is_ok());
        assert!(apply_manual_tax(t, Some(d("46")), rate(), false, 0).is_err());
    }

    #[test]
    fn duplicate_example() {
        let t = example_two();
        assert_eq!(
            (t.subtotal, t.tax_amount, t.total),
            (d("952"), d("48"), d("1000"))
        );
        let m = apply_manual_tax(t, Some(d("47")), rate(), true, 0).unwrap();
        assert_eq!(
            (m.subtotal, m.tax_amount, m.total),
            (d("953"), d("47"), d("1000"))
        );
        assert_eq!(m.subtotal.to_string(), "953.0000");
    }

    #[test]
    fn manual_tax_rules() {
        let t = example_one();
        assert_eq!(
            apply_manual_tax(t, Some(d("48.5")), rate(), false, 0),
            Err("稅額必須是整數".into())
        );
        assert_eq!(
            apply_manual_tax(t, Some(d("48.123")), rate(), false, 2),
            Err("稅額最多 2 位小數".into())
        );
        assert!(apply_manual_tax(t, Some(d("48.0000")), rate(), false, 0).is_ok());
        let zero = totals([d("500")], Decimal::ZERO, false, 0);
        assert_eq!(
            apply_manual_tax(zero, Some(d("0")), Decimal::ZERO, false, 0),
            Err(MSG_TAX_ON_ZERO_RATE.into())
        );
        assert_eq!(
            apply_manual_tax(zero, None, Decimal::ZERO, false, 0),
            Ok(zero)
        );
    }

    #[test]
    fn entered_tax_rules() {
        assert!(check_entered_tax(d("967"), d("48"), rate(), 0).is_ok());
        assert!(check_entered_tax(d("967"), d("49"), rate(), 0).is_ok());
        assert_eq!(
            check_entered_tax(d("967"), d("50"), rate(), 0),
            Err("稅額與未稅金額 × 稅率相差超過 1 元（應為 48）".into())
        );
        assert!(check_entered_tax(d("100"), d("0"), Decimal::ZERO, 0).is_ok());
        assert_eq!(
            check_entered_tax(d("100"), d("1"), Decimal::ZERO, 0),
            Err(MSG_TAX_ON_ZERO_RATE.into())
        );
        assert_eq!(
            check_entered_tax(d("100"), d("5.5"), rate(), 0),
            Err("稅額必須是整數".into())
        );
    }

    #[test]
    fn displays_amounts() {
        assert_eq!(display(d("48.0000"), 0), "48");
        assert_eq!(display(d("48.5"), 2), "48.50");
        assert_eq!(display_grouped(d("1050.0000"), 0), "1,050");
        assert_eq!(display_grouped(d("1234567.5"), 2), "1,234,567.50");
        assert_eq!(display_grouped(d("999"), 0), "999");
        assert_eq!(display_grouped(d("-1050"), 0), "-1,050");
    }
}
