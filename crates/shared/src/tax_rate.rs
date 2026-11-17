//! 稅別（docs/architecture.md §8.6 `tax_rates`）。
//!
//! | 端點 | 權限 | response |
//! |---|---|---|
//! | `GET /api/v1/tax-rates` | 登入 | `TaxRate[]`（全部，不分頁，依稅率由高到低、再依代碼排序） |
//!
//! 由 migration 寫入，M3 不提供修改。代碼與商品的 `tax_type` 選項相同。

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use specta::Type;

/// 稅別。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct TaxRate {
    /// `taxable` / `zero_rated` / `exempt`。
    pub code: String,
    /// 應稅 / 零稅率 / 免稅。
    pub name: String,
    /// 稅率，例如 `"0.0500"`。
    #[serde(with = "crate::decimal::serde_str")]
    #[specta(type = String)]
    pub rate: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tax_rate_json_shape() {
        let tax = TaxRate {
            code: "taxable".into(),
            name: "應稅".into(),
            rate: Decimal::new(500, 4),
        };
        assert_eq!(
            serde_json::to_value(&tax).unwrap(),
            json!({ "code": "taxable", "name": "應稅", "rate": "0.0500" })
        );
    }
}
