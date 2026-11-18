//! 銷貨單據的金額與稅額計算（docs/architecture.md §8.9「金額與稅額」）。純邏輯，不查詢資料庫。
//!
//! 訂單、出貨單、銷貨退回使用同一套計算，四捨五入一律 `MidpointAwayFromZero`（與
//! [`crate::purchasing::amounts`] 共用）。銷貨以台幣交易，`d` 為台幣的 `currencies.decimal_places`。
//!
//! | 值 | 未稅（`tax_included = false`） | 含稅（`tax_included = true`） |
//! |---|---|---|
//! | 明細 `amount` | `round(qty × unit_price × (1 − discount_rate), d)` | 同左 |
//! | `total` | `subtotal + tax_amount` | `Σ amount` |
//! | `subtotal` | `Σ amount` | `round(total ÷ (1 + 稅率), d)` |
//! | `tax_amount` | `round(subtotal × 稅率, d)` | `total − subtotal` |
//!
//! 回傳的值都補齊到 4 位小數（`NUMERIC(18,4)`），與從資料庫讀出的字串相同。

use hammer_shared::decimal::Decimal;

use crate::purchasing::amounts::{AMOUNT_SCALE, round_to, with_scale};

/// 稅別：應稅（新增時沒有其他依據的預設值）。
pub const TAXABLE: &str = "taxable";

/// 明細金額：`round(qty × unit_price × (1 − discount_rate), decimal_places)`，4 位小數。
pub fn line_amount(
    qty: Decimal,
    unit_price: Decimal,
    discount_rate: Decimal,
    decimal_places: u32,
) -> Decimal {
    round_to(
        qty * unit_price * (Decimal::ONE - discount_rate),
        decimal_places,
        AMOUNT_SCALE,
    )
}

/// 單據的合計（4 位小數）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// 未稅金額。
    pub subtotal: Decimal,
    /// 稅額。
    pub tax_amount: Decimal,
    /// 總計。
    pub total: Decimal,
}

/// 依明細金額（[`line_amount`] 的結果）計算合計。稅額以整張單據計算；`tax_included` 為表頭的「單價含稅」。
pub fn totals(
    amounts: impl IntoIterator<Item = Decimal>,
    tax_rate: Decimal,
    tax_included: bool,
    decimal_places: u32,
) -> Totals {
    let sum: Decimal = amounts.into_iter().sum();
    if tax_included {
        let total = with_scale(sum, AMOUNT_SCALE);
        let subtotal = round_to(
            total / (Decimal::ONE + tax_rate),
            decimal_places,
            AMOUNT_SCALE,
        );
        Totals {
            subtotal,
            tax_amount: with_scale(total - subtotal, AMOUNT_SCALE),
            total,
        }
    } else {
        let subtotal = with_scale(sum, AMOUNT_SCALE);
        let tax_amount = round_to(subtotal * tax_rate, decimal_places, AMOUNT_SCALE);
        Totals {
            subtotal,
            tax_amount,
            total: with_scale(subtotal + tax_amount, AMOUNT_SCALE),
        }
    }
}

/// 新增時沒有帶稅別的預設值：明細商品的稅別全部相同時為該值，否則（含沒有明細）為 `taxable`。
pub fn default_tax_type<'a>(product_tax_types: impl IntoIterator<Item = &'a str>) -> &'a str {
    let mut types = product_tax_types.into_iter();
    match types.next() {
        Some(first) if types.all(|t| t == first) => first,
        _ => TAXABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn s(v: Decimal) -> String {
        v.to_string()
    }

    #[test]
    fn example_tax_excluded_taxable() {
        // §8.9 範例一：未稅、應稅（d = 0、稅率 0.05）
        let a1 = line_amount(d("3"), d("33.3333"), d("0"), 0);
        let a2 = line_amount(d("10"), d("120.0000"), d("0.15"), 0);
        assert_eq!(s(a1), "100.0000");
        assert_eq!(s(a2), "1020.0000");
        let t = totals([a1, a2], d("0.0500"), false, 0);
        assert_eq!(s(t.subtotal), "1120.0000");
        assert_eq!(s(t.tax_amount), "56.0000");
        assert_eq!(s(t.total), "1176.0000");
    }

    #[test]
    fn example_tax_included_taxable() {
        // §8.9 範例二：含稅、應稅（d = 0、稅率 0.05）
        let a1 = line_amount(d("2"), d("105.0000"), d("0"), 0);
        let a2 = line_amount(d("5"), d("99.0000"), d("0.1"), 0);
        assert_eq!(s(a1), "210.0000");
        assert_eq!(s(a2), "446.0000");
        let t = totals([a1, a2], d("0.0500"), true, 0);
        assert_eq!(s(t.total), "656.0000");
        assert_eq!(s(t.subtotal), "625.0000");
        assert_eq!(s(t.tax_amount), "31.0000");
    }

    #[test]
    fn example_tax_included_exempt() {
        // §8.9 範例三：含稅、免稅（稅率 0）
        let t = totals([d("200.0000"), d("300.0000")], d("0"), true, 0);
        assert_eq!(s(t.total), "500.0000");
        assert_eq!(s(t.subtotal), "500.0000");
        assert_eq!(s(t.tax_amount), "0.0000");
    }

    #[test]
    fn zero_rate_is_same_with_or_without_tax() {
        for included in [false, true] {
            let t = totals([d("123.0000")], d("0"), included, 0);
            assert_eq!(s(t.subtotal), "123.0000");
            assert_eq!(s(t.tax_amount), "0.0000");
            assert_eq!(s(t.total), "123.0000");
        }
    }

    #[test]
    fn midpoint_rounds_away_from_zero() {
        // 明細：2.5 → 3（不是銀行家捨入的 2）
        assert_eq!(s(line_amount(d("5"), d("0.5"), d("0"), 0)), "3.0000");
        // 折扣後 0.5 → 1
        assert_eq!(s(line_amount(d("1"), d("1"), d("0.5"), 0)), "1.0000");
        // 未稅：稅額 10 × 0.05 = 0.5 → 1
        let t = totals([d("10")], d("0.05"), false, 0);
        assert_eq!(s(t.tax_amount), "1.0000");
        assert_eq!(s(t.total), "11.0000");
        // 含稅：21 ÷ 1.05 = 20 整除；52.5 ÷ 1.05 = 50 → 稅額 2.5 不捨入（total − subtotal）
        let t = totals([d("52.5")], d("0.05"), true, 2);
        assert_eq!(s(t.subtotal), "50.0000");
        assert_eq!(s(t.tax_amount), "2.5000");
        // 含稅：10.5 ÷ 1.05 = 10；1 ÷ 1.05 = 0.952… → 1
        let t = totals([d("1")], d("0.05"), true, 0);
        assert_eq!(s(t.subtotal), "1.0000");
        assert_eq!(s(t.tax_amount), "0.0000");
    }

    #[test]
    fn decimal_places_follow_currency_setting() {
        // 台幣設定為 2 位小數時
        assert_eq!(s(line_amount(d("3"), d("33.3333"), d("0"), 2)), "100.0000");
        assert_eq!(s(line_amount(d("1"), d("10.005"), d("0"), 2)), "10.0100");
        let t = totals([d("10.01")], d("0.05"), false, 2);
        assert_eq!(s(t.tax_amount), "0.5000");
        assert_eq!(s(t.total), "10.5100");
        let t = totals([d("10.51")], d("0.05"), true, 2);
        assert_eq!(s(t.subtotal), "10.0100");
        assert_eq!(s(t.tax_amount), "0.5000");
    }

    #[test]
    fn full_discount_is_zero() {
        assert_eq!(s(line_amount(d("4"), d("99"), d("1"), 0)), "0.0000");
    }

    #[test]
    fn no_lines_total_zero() {
        for included in [false, true] {
            let t = totals([], d("0.05"), included, 0);
            assert_eq!(s(t.subtotal), "0.0000");
            assert_eq!(s(t.tax_amount), "0.0000");
            assert_eq!(s(t.total), "0.0000");
        }
    }

    #[test]
    fn default_tax_type_follows_products() {
        assert_eq!(default_tax_type([]), TAXABLE);
        assert_eq!(default_tax_type(["exempt"]), "exempt");
        assert_eq!(default_tax_type(["zero_rated", "zero_rated"]), "zero_rated");
        assert_eq!(default_tax_type(["zero_rated", "exempt"]), TAXABLE);
    }
}
