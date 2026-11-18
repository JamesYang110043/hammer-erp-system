//! 訂單的確認、手動結案、作廢（docs/architecture.md §8.9「訂單的狀態」），掛在 [`crate::documents`] 的
//! 框架上（[`SalesOrderHandler`]）。與採購單（[`crate::purchasing::orders`]）相同：
//!
//! | 動作 | 前一個狀態 → 之後 | 寫入 |
//! |---|---|---|
//! | 確認 | `draft` → `confirmed` | `posted_at`、`posted_by` |
//! | 手動結案 | `confirmed`、`partially_shipped` → `closed` | `closed_at`、`closed_by` |
//! | 作廢 | `confirmed` → `void` | `voided_at`、`voided_by` |
//!
//! 訂單不支援過帳（`404`）；有出貨（`partially_shipped`）時不可作廢，請手動結案。
//!
//! # 確認時的檢查
//!
//! 鎖定順序：訂單（框架已 `FOR UPDATE`）→ 客戶（`FOR SHARE`）→ 明細（`FOR UPDATE`）→ 明細的商品
//! （`FOR SHARE`，之後到結案前商品不可刪除、改為服務、修改單位，§8.9「客戶、商品被銷貨單據引用時的
//! 限制」）。錯誤一律 `422 VALIDATION`，全部檢查完一起回傳：
//!
//! - 客戶未刪除（`customer_id`「客戶已刪除」）
//! - 至少一列明細（「至少要有一筆明細」）
//! - 每一列：商品未刪除（「商品已刪除」）、`qty > 0`、`unit_price ≥ 0`、`0 ≤ discount_rate ≤ 1`
//!
//! 不檢查庫存、信用額度；金額不重新計算。

use hammer_db::document::Stamp;
use hammer_db::inventory as inventory_db;
use hammer_db::sales as db;
use hammer_db::sqlx::PgConnection;
use hammer_shared::document::{DocumentAction, DocumentStatus};

use super::SALES_ORDER_LINE;
use super::save::{CUSTOMER_ID, line_errors};
use crate::CoreError;
use crate::documents::{DocumentContext, DocumentHandler, HandlerFuture, Transition};
use crate::purchasing::save::id_of;
use crate::record_data::FieldError;
use crate::record_lines::LineError;
use crate::stock_adjustments::{MSG_NO_LINES, MSG_PRODUCT_DELETED};

/// 客戶已刪除。
pub const MSG_CUSTOMER_DELETED: &str = "客戶已刪除";

const PRODUCT_ID: &str = "product_id";

/// 訂單的處理函式。
#[derive(Debug, Default, Clone, Copy)]
pub struct SalesOrderHandler;

/// 訂單的狀態轉換：確認、結案、作廢；不支援過帳。
pub fn order_transition(action: DocumentAction) -> Option<Transition> {
    match action {
        DocumentAction::Confirm => Some(Transition {
            from: &[DocumentStatus::Draft],
            to: DocumentStatus::Confirmed,
            stamp: Stamp::Posted,
        }),
        DocumentAction::Close => Some(Transition {
            from: &[DocumentStatus::Confirmed, DocumentStatus::PartiallyShipped],
            to: DocumentStatus::Closed,
            stamp: Stamp::Closed,
        }),
        DocumentAction::Void => Some(Transition {
            from: &[DocumentStatus::Confirmed],
            to: DocumentStatus::Void,
            stamp: Stamp::Voided,
        }),
        DocumentAction::Post => None,
    }
}

impl DocumentHandler for SalesOrderHandler {
    fn transition(&self, action: DocumentAction) -> Option<Transition> {
        order_transition(action)
    }

    fn post<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(async { Err(CoreError::Internal("訂單不支援過帳".into())) })
    }

    /// 作廢不影響庫存，也不需要其他檢查（只有 `confirmed` 可以作廢，由狀態轉換保證沒有出貨）。
    fn void<'a>(
        &'a self,
        _conn: &'a mut PgConnection,
        _doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn confirm<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        doc: &'a DocumentContext<'a>,
    ) -> HandlerFuture<'a> {
        Box::pin(confirm(conn, doc))
    }
}

async fn confirm(conn: &mut PgConnection, doc: &DocumentContext<'_>) -> Result<(), CoreError> {
    let mut fields = Vec::new();
    let customer_deleted = match id_of(doc.data.get(CUSTOMER_ID)) {
        Some(id) => db::lock_customer(&mut *conn, id).await? != Some(false),
        None => true,
    };
    if customer_deleted {
        fields.push(FieldError {
            field_key: CUSTOMER_ID.into(),
            message: MSG_CUSTOMER_DELETED.into(),
        });
    }

    let rows = db::list_order_lines_for_update(&mut *conn, doc.record_id).await?;
    let mut lines = Vec::new();
    if rows.is_empty() {
        lines.push(LineError::detail(SALES_ORDER_LINE, MSG_NO_LINES));
    }
    let mut ids: Vec<_> = rows.iter().map(|r| r.product_id).collect();
    ids.sort_unstable();
    ids.dedup();
    let products = inventory_db::lock_products(&mut *conn, &ids).await?;
    for (index, row) in rows.iter().enumerate() {
        let alive = products
            .iter()
            .any(|p| p.id == row.product_id && !p.deleted);
        if !alive {
            lines.push(LineError::field(
                SALES_ORDER_LINE,
                index,
                PRODUCT_ID,
                MSG_PRODUCT_DELETED,
            ));
        }
        lines.extend(LineError::from_fields(
            SALES_ORDER_LINE,
            index,
            line_errors(Some(row.qty), Some(row.unit_price), Some(row.discount_rate)),
        ));
    }

    match (fields.is_empty(), lines.is_empty()) {
        (true, true) => Ok(()),
        (false, true) => Err(CoreError::InvalidFields(fields)),
        _ => Err(CoreError::InvalidLines { fields, lines }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_transitions_follow_status_table() {
        let confirm = order_transition(DocumentAction::Confirm).unwrap();
        assert_eq!(confirm.from, [DocumentStatus::Draft]);
        assert_eq!(confirm.to, DocumentStatus::Confirmed);
        assert_eq!(confirm.stamp, Stamp::Posted);

        let close = order_transition(DocumentAction::Close).unwrap();
        assert_eq!(
            close.from,
            [DocumentStatus::Confirmed, DocumentStatus::PartiallyShipped]
        );
        assert_eq!(close.to, DocumentStatus::Closed);
        assert_eq!(close.stamp, Stamp::Closed);

        let void = order_transition(DocumentAction::Void).unwrap();
        assert_eq!(void.from, [DocumentStatus::Confirmed]);
        assert_eq!(void.to, DocumentStatus::Void);
        assert_eq!(void.stamp, Stamp::Voided);

        // 訂單沒有過帳
        assert!(order_transition(DocumentAction::Post).is_none());
        assert!(SalesOrderHandler.transition(DocumentAction::Post).is_none());
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        use DocumentStatus::*;
        let allowed = |action, status: DocumentStatus| {
            order_transition(action).is_some_and(|t| t.from.contains(&status))
        };
        assert!(allowed(DocumentAction::Confirm, Draft));
        assert!(!allowed(DocumentAction::Close, Draft));
        assert!(!allowed(DocumentAction::Void, Draft));
        assert!(!allowed(DocumentAction::Confirm, Confirmed));
        // 部分出貨不可作廢，只能結案
        assert!(!allowed(DocumentAction::Void, PartiallyShipped));
        assert!(allowed(DocumentAction::Close, PartiallyShipped));
        // 採購單的「部分進貨」不屬於訂單
        assert!(!allowed(DocumentAction::Close, PartiallyReceived));
        for action in [
            DocumentAction::Confirm,
            DocumentAction::Close,
            DocumentAction::Void,
        ] {
            assert!(!allowed(action, Closed));
            assert!(!allowed(action, Void));
        }
    }
}
