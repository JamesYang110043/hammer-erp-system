//! 進貨單據的金額與稅額計算（docs/architecture.md §8.8「金額與稅額」）。純邏輯，不查詢資料庫。
//!
//! 採購單、進貨單、進貨退回使用同一套計算，四捨五入一律 `MidpointAwayFromZero`：
//!
//! | 值 | 計算 |
//! |---|---|
//! | 明細 `amount` | `round(qty × unit_price, d)` |
//! | `subtotal` | `Σ amount` |
//! | `tax_amount` | `round(subtotal × 稅率, d)` |
//! | `total` | `subtotal + tax_amount` |
//! | `total_twd` | `round(total × exchange_rate, 2)`；匯率留空時為 `None` |
//! | 台幣單位成本 | `round(unit_price × exchange_rate, 4)`（過帳時寫入，稅額不計入成本） |
//!
//! `d` 為單據幣別的 `currencies.decimal_places`（TWD、JPY 為 0，USD 為 2）。回傳的值已補齊資料表的小數
//! 位數（單據幣別的金額 4 位、台幣合計 2 位、單位成本 4 位），與從資料庫讀出的字串相同。

use hammer_shared::decimal::{Decimal, RoundingStrategy};

/// 單據幣別金額（`NUMERIC(18,4)`）的小數位數。
pub const AMOUNT_SCALE: u32 = 4;
/// 台幣合計 `total_twd`（`NUMERIC(18,2)`）的小數位數。
pub const TWD_SCALE: u32 = 2;
/// 台幣單位成本 `unit_cost_twd`（`NUMERIC(18,4)`）的小數位數。
pub const COST_SCALE: u32 = 4;
/// 匯率（`NUMERIC(18,6)`）的小數位數。
pub const RATE_SCALE: u32 = 6;

/// 稅別：應稅。
pub const TAXABLE: &str = "taxable";
/// 稅別：零稅率。
pub const ZERO_RATED: &str = "zero_rated";

/// 四捨五入到 `dp` 位，並補齊到 `scale` 位小數（`dp ≤ scale`）。
fn round_to(value: Decimal, dp: u32, scale: u32) -> Decimal {
    let mut v = value.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero);
    v.rescale(scale);
    v
}

/// 值補齊到 `scale` 位小數（不四捨五入超過的位數，呼叫端保證不超過）。
pub fn with_scale(value: Decimal, scale: u32) -> Decimal {
    let mut v = value;
    v.rescale(scale);
    v
}

/// 明細金額：`round(qty × unit_price, decimal_places)`，4 位小數。
pub fn line_amount(qty: Decimal, unit_price: Decimal, decimal_places: u32) -> Decimal {
    round_to(qty * unit_price, decimal_places, AMOUNT_SCALE)
}

/// 單據的合計。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    /// 未稅金額（4 位小數）。
    pub subtotal: Decimal,
    /// 稅額（4 位小數）。
    pub tax_amount: Decimal,
    /// 總計（4 位小數）。
    pub total: Decimal,
    /// 台幣總計（2 位小數）；匯率留空時為 `None`。
    pub total_twd: Option<Decimal>,
}

/// 依明細金額（[`line_amount`] 的結果）計算合計。稅額以整張單據計算。
pub fn totals(
    amounts: impl IntoIterator<Item = Decimal>,
    tax_rate: Decimal,
    decimal_places: u32,
    exchange_rate: Option<Decimal>,
) -> Totals {
    let subtotal = with_scale(amounts.into_iter().sum(), AMOUNT_SCALE);
    let tax_amount = round_to(subtotal * tax_rate, decimal_places, AMOUNT_SCALE);
    let total = with_scale(subtotal + tax_amount, AMOUNT_SCALE);
    let total_twd = exchange_rate.map(|rate| round_to(total * rate, TWD_SCALE, TWD_SCALE));
    Totals {
        subtotal,
        tax_amount,
        total,
        total_twd,
    }
}

/// 台幣單位成本：`round(unit_price × exchange_rate, 4)`。
pub fn unit_cost_twd(unit_price: Decimal, exchange_rate: Decimal) -> Decimal {
    round_to(unit_price * exchange_rate, COST_SCALE, COST_SCALE)
}

/// 新增時沒有帶稅別的預設值：本位幣（TWD）為應稅，外幣為零稅率（進口的營業稅在報關時另外繳納）。
pub fn default_tax_type(is_base_currency: bool) -> &'static str {
    if is_base_currency {
        TAXABLE
    } else {
        ZERO_RATED
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    /// 字串比對：確認小數位數也正確。
    fn s(v: Decimal) -> String {
        v.to_string()
    }

    #[test]
    fn example_twd_taxable() {
        // §8.8 範例一：台幣、應稅（d = 0、稅率 0.05、匯率 1）
        let a1 = line_amount(d("3"), d("33.3333"), 0);
        let a2 = line_amount(d("2"), d("150.5000"), 0);
        assert_eq!(s(a1), "100.0000");
        assert_eq!(s(a2), "301.0000");
        let t = totals([a1, a2], d("0.0500"), 0, Some(d("1.000000")));
        assert_eq!(s(t.subtotal), "401.0000");
        assert_eq!(s(t.tax_amount), "20.0000");
        assert_eq!(s(t.total), "421.0000");
        assert_eq!(t.total_twd.map(s).as_deref(), Some("421.00"));
        assert_eq!(s(unit_cost_twd(d("33.3333"), d("1.000000"))), "33.3333");
    }

    #[test]
    fn example_usd_zero_rated() {
        // §8.8 範例二：美元、零稅率（d = 2、匯率 32.150000）
        let rate = d("32.150000");
        let a1 = line_amount(d("10"), d("12.3450"), 2);
        let a2 = line_amount(d("3"), d("7.5000"), 2);
        assert_eq!(s(a1), "123.4500");
        assert_eq!(s(a2), "22.5000");
        let t = totals([a1, a2], d("0"), 2, Some(rate));
        assert_eq!(s(t.subtotal), "145.9500");
        assert_eq!(s(t.tax_amount), "0.0000");
        assert_eq!(s(t.total), "145.9500");
        assert_eq!(t.total_twd.map(s).as_deref(), Some("4692.29"));
        assert_eq!(s(unit_cost_twd(d("12.3450"), rate)), "396.8918");
        assert_eq!(s(unit_cost_twd(d("7.5000"), rate)), "241.1250");
    }

    #[test]
    fn foreign_currency_taxable_rounds_tax_to_currency_places() {
        // 美元、應稅：稅額 round(10.01 × 0.05, 2) = round(0.5005, 2) = 0.50
        let a = line_amount(d("1"), d("10.0100"), 2);
        let t = totals([a], d("0.05"), 2, Some(d("30.5")));
        assert_eq!(s(t.tax_amount), "0.5000");
        assert_eq!(s(t.total), "10.5100");
        assert_eq!(t.total_twd.map(s).as_deref(), Some("320.56"));
    }

    #[test]
    fn midpoint_rounds_away_from_zero() {
        // 0.5 → 1（不是銀行家捨入的 0）、2.5 → 3
        assert_eq!(s(line_amount(d("1"), d("0.5"), 0)), "1.0000");
        assert_eq!(s(line_amount(d("5"), d("0.5"), 0)), "3.0000");
        // 稅額：subtotal 10 × 0.05 = 0.5 → 1
        let t = totals([d("10.0000")], d("0.05"), 0, None);
        assert_eq!(s(t.tax_amount), "1.0000");
        assert_eq!(s(t.total), "11.0000");
        // total_twd：1.005 → 1.01
        let t = totals([d("1.0050")], d("0"), 4, Some(d("1")));
        assert_eq!(t.total_twd.map(s).as_deref(), Some("1.01"));
        // 成本：0.00005 → 0.0001
        assert_eq!(s(unit_cost_twd(d("0.0001"), d("0.5"))), "0.0001");
    }

    #[test]
    fn untaxed_and_exempt_have_no_tax() {
        let a = line_amount(d("7"), d("99.9900"), 0);
        assert_eq!(s(a), "700.0000");
        let t = totals([a], d("0.0000"), 0, Some(d("1")));
        assert_eq!(s(t.tax_amount), "0.0000");
        assert_eq!(s(t.total), "700.0000");
    }

    #[test]
    fn missing_rate_leaves_twd_total_empty() {
        let t = totals([d("12.3400")], d("0"), 2, None);
        assert_eq!(t.total_twd, None);
        assert_eq!(s(t.total), "12.3400");
    }

    #[test]
    fn no_lines_total_zero() {
        let t = totals([], d("0.05"), 0, Some(d("1")));
        assert_eq!(s(t.subtotal), "0.0000");
        assert_eq!(s(t.total), "0.0000");
        assert_eq!(t.total_twd.map(s).as_deref(), Some("0.00"));
    }

    #[test]
    fn four_decimal_currency_keeps_precision() {
        assert_eq!(s(line_amount(d("3"), d("0.3333"), 4)), "0.9999");
        assert_eq!(s(line_amount(d("0.0001"), d("0.0001"), 4)), "0.0000");
    }

    #[test]
    fn default_tax_type_depends_on_currency() {
        assert_eq!(default_tax_type(true), TAXABLE);
        assert_eq!(default_tax_type(false), ZERO_RATED);
    }
}
