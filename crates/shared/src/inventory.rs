//! 庫存的共通型別（`docs/architecture.md` §8.10）。

use serde::{Deserialize, Serialize};
use specta::Type;

/// 批號的最大長度（字元數，§8.10「批號」）。
pub const LOT_NO_MAX_CHARS: usize = 50;

/// 庫存異動的類型（`inventory_transactions.txn_type`）。JSON 中為 snake_case。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum TxnType {
    /// 進貨入庫。
    Receipt,
    /// 出貨。
    Shipment,
    /// 銷貨退回。
    SalesReturn,
    /// 進貨退回。
    PurchaseReturn,
    /// 盤點與調整（含期初庫存）。
    Adjustment,
    /// 作廢單據產生的反向異動。
    VoidReversal,
}

impl TxnType {
    /// 所有類型。
    pub const ALL: [Self; 6] = [
        Self::Receipt,
        Self::Shipment,
        Self::SalesReturn,
        Self::PurchaseReturn,
        Self::Adjustment,
        Self::VoidReversal,
    ];

    /// 資料庫與 JSON 中的值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Shipment => "shipment",
            Self::SalesReturn => "sales_return",
            Self::PurchaseReturn => "purchase_return",
            Self::Adjustment => "adjustment",
            Self::VoidReversal => "void_reversal",
        }
    }

    /// 由資料庫的值解析；不認得的值回傳 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txn_type_round_trips() {
        for t in TxnType::ALL {
            assert_eq!(TxnType::parse(t.as_str()), Some(t));
            assert_eq!(
                serde_json::to_value(t).unwrap(),
                serde_json::json!(t.as_str())
            );
        }
        assert_eq!(TxnType::parse("transfer"), None);
    }
}
